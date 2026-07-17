//! Загрузка/удаление/просмотр/реиндексация базы знаний (RAG) командами
//! `/rag add|remove|list|rebuild` (spec §9.3). Индексация и реиндексация —
//! отменяемые фоновые задачи с прогрессом; удаление и список — быстрые операции
//! БД на месте. Всё с изоляцией по `profile_id`.

use std::collections::HashMap;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::app::events::{AppEvent, RagProgress};
use crate::features::tools::rag::ChunkParams;
use crate::shared::api::Embedder;
use crate::shared::i18n::Locale;
use crate::shared::storage::Storage;

use super::Orchestrator;

/// Максимум чанков в одном запросе к эмбеддеру. Ограничивает размер каждого запроса
/// и даёт прогресс по мере готовности чанков (баннер двигается *в ходе* эмбеддинга
/// крупного файла). Файл с ≤16 чанками по-прежнему эмбеддится одним запросом (как
/// раньше) — поведение для маленьких файлов не меняется.
const EMBED_BATCH_CHUNKS: usize = 16;

impl Orchestrator {
    /// Профиль активного чата (RAG изолирован по `profile_id`, §9.5). `None` — нет
    /// активного чата.
    pub(super) fn active_profile_id(&self) -> Option<Uuid> {
        self.active_id
            .and_then(|id| self.chats.iter().find(|c| c.id == id))
            .map(|c| c.profile_id)
    }

    /// Параметры чанкинга из текущих настроек (`config.rag`).
    fn chunk_params(&self) -> ChunkParams {
        ChunkParams::from_settings(&self.config.rag)
    }

    /// Запускает фоновую индексацию файла/директории в RAG активного профиля
    /// (команда `/rag add`, spec §9.3). Сканирование, чтение, эмбеддинг и запись
    /// идут в отдельной задаче; прогресс — событиями [`RagProgress`]. Предыдущая
    /// незавершённая индексация отменяется (одна за раз).
    pub(super) fn handle_rag_add(&mut self, path: String, recursive: bool) {
        let path = path.trim().to_string();
        if path.is_empty() {
            return;
        }
        let Some(profile_id) = self.active_profile_id() else {
            self.fail_rag(self.ui_locale().t("ui.err.rag_no_active_chat"));
            return;
        };

        let cancel = self.reset_rag_cancel();
        spawn_rag_ingest(RagIngest {
            embedder: self.engines.embedder(),
            storage: self.storage.clone(),
            profile_id,
            root: std::path::PathBuf::from(path),
            recursive,
            params: self.chunk_params(),
            cancel,
            loc: self.ui_locale(),
            evt_tx: self.evt_tx.clone(),
        });
    }

    /// Удаляет из RAG активного профиля файл или директорию (со всем, что под ней)
    /// по пути (команда `/rag remove`, spec §9.3). Операция быстрая (только БД, без
    /// эмбеддинга), поэтому выполняется на месте. Если путь есть на диске — берём его
    /// канонический ключ (как при добавлении); иначе сопоставляем по введённой строке
    /// (БД сама нормализует разделители/регистр), что позволяет чистить записи уже
    /// удалённых с диска файлов.
    pub(super) fn handle_rag_delete(&mut self, path: String) {
        let path = path.trim().to_string();
        if path.is_empty() {
            return;
        }
        let Some(profile_id) = self.active_profile_id() else {
            self.fail_rag(self.ui_locale().t("ui.err.rag_no_active_chat"));
            return;
        };
        let p = std::path::Path::new(&path);
        let needle = if p.exists() {
            crate::features::rag_ingest::canonical_source(p)
        } else {
            path.clone()
        };
        let progress = match self.storage.db().rag_delete_under(profile_id, &needle) {
            Ok(chunks) => RagProgress::Removed { chunks },
            Err(err) => RagProgress::Failed(
                self.ui_locale()
                    .tf("ui.err.rag_delete_failed", &[("err", &err.to_string())]),
            ),
        };
        let _ = self.evt_tx.send(AppEvent::RagProgress(progress));
    }

    /// Показывает источники базы знаний активного профиля (команда `/rag list`):
    /// по каждому источнику — число чанков и дата. Быстрая операция БД на месте.
    pub(super) fn handle_rag_list(&mut self) {
        let Some(profile_id) = self.active_profile_id() else {
            self.fail_rag(self.ui_locale().t("ui.err.rag_no_active_chat"));
            return;
        };
        let progress = match self.storage.db().rag_list_sources(profile_id) {
            Ok(sources) => RagProgress::Listed { sources },
            Err(err) => RagProgress::Failed(
                self.ui_locale()
                    .tf("ui.err.rag_read_kb_failed", &[("err", &err.to_string())]),
            ),
        };
        let _ = self.evt_tx.send(AppEvent::RagProgress(progress));
    }

    /// Реиндексирует базу знаний активного профиля (команда `/rag rebuild`, spec
    /// §9.3): перечанковывает и переэмбеддивает сохранённые исходники текущими
    /// параметрами/embedding-моделью. Нужна после смены размера чанка/перекрытия или
    /// embedding-модели (другая размерность). Идёт фоновой задачей; отменяет
    /// предыдущую RAG-операцию (одна за раз).
    pub(super) fn handle_rag_rebuild(&mut self) {
        let Some(profile_id) = self.active_profile_id() else {
            self.fail_rag(self.ui_locale().t("ui.err.rag_no_active_chat"));
            return;
        };
        let cancel = self.reset_rag_cancel();
        spawn_rag_rebuild(RagRebuild {
            embedder: self.engines.embedder(),
            storage: self.storage.clone(),
            profile_id,
            params: self.chunk_params(),
            cancel,
            loc: self.ui_locale(),
            evt_tx: self.evt_tx.clone(),
        });
    }

    /// Отменяет предыдущую фоновую RAG-задачу (если шла) и заводит новый токен.
    fn reset_rag_cancel(&mut self) -> CancellationToken {
        if let Some(token) = self.rag_cancel.take() {
            token.cancel();
        }
        let cancel = CancellationToken::new();
        self.rag_cancel = Some(cancel.clone());
        cancel
    }

    /// Шлёт ошибку RAG-операции в UI (баннер/заметка).
    fn fail_rag(&self, msg: &str) {
        let _ = self
            .evt_tx
            .send(AppEvent::RagProgress(RagProgress::Failed(msg.to_string())));
    }
}

/// Параметры фоновой задачи индексации файлов в RAG (`/rag add`).
struct RagIngest {
    embedder: Arc<dyn Embedder>,
    storage: Arc<Storage>,
    profile_id: Uuid,
    root: std::path::PathBuf,
    recursive: bool,
    params: ChunkParams,
    cancel: CancellationToken,
    /// Язык интерфейса (ось B) — для сообщений о прогрессе/ошибках, видимых человеку.
    loc: &'static Locale,
    evt_tx: tokio::sync::mpsc::UnboundedSender<AppEvent>,
}

/// Запускает фоновую индексацию (spec §9.3): сканирует путь, проверяет доступность
/// эмбеддера, затем по очереди читает/чанкует/эмбеддит/пишет каждый файл, эмитя
/// [`RagProgress`]. Отменяемо по `cancel` (между файлами). Storage потокобезопасен
/// (внутренний мьютекс), эмбеддинг асинхронен — задача не блокирует оркестратор.
fn spawn_rag_ingest(task: RagIngest) {
    let RagIngest {
        embedder,
        storage,
        profile_id,
        root,
        recursive,
        params,
        cancel,
        loc,
        evt_tx,
    } = task;

    tokio::spawn(async move {
        let send = |p: RagProgress| {
            let _ = evt_tx.send(AppEvent::RagProgress(p));
        };

        // 1. Сканируем файлы (txt/md/html). Ошибка пути / пустой результат — понятный отказ.
        let files = match crate::features::rag_ingest::scan(&root, recursive) {
            Ok(files) => files,
            Err(err) => {
                send(RagProgress::Failed(loc.tf(
                    "ui.err.rag_path_unavailable",
                    &[("err", &err.to_string())],
                )));
                return;
            }
        };
        if files.is_empty() {
            send(RagProgress::Failed(loc.t("ui.err.rag_no_files").into()));
            return;
        }

        // 2. Предпроверка эмбеддера — быстрый понятный отказ, если RAG не настроен.
        if let Err(err) = embedder.embed(vec!["ping".into()]).await {
            send(RagProgress::Failed(loc.tf(
                "ui.err.rag_embedder_unavailable",
                &[("err", &err.to_string())],
            )));
            return;
        }

        let total = files.len();
        send(RagProgress::Started { total });

        let mut chunks_total = 0usize;
        let mut errors = 0usize;
        for (i, file) in files.iter().enumerate() {
            if cancel.is_cancelled() {
                break;
            }
            let (name, dir) = display_parts(file);
            // Файл начат (чанкинг ещё впереди) — chunks_total=0 (баннер как раньше).
            send(RagProgress::Indexing {
                index: i + 1,
                total,
                name: name.clone(),
                dir: dir.clone(),
                chunks_done: 0,
                chunks_total: 0,
            });
            // Прогресс по чанкам: клонируем имя/папку в замыкание (FnMut вызывается
            // многократно, а `send` заимствует `evt_tx`).
            let progress = |done: usize, tot: usize| {
                let _ = evt_tx.send(AppEvent::RagProgress(RagProgress::Indexing {
                    index: i + 1,
                    total,
                    name: name.clone(),
                    dir: dir.clone(),
                    chunks_done: done,
                    chunks_total: tot,
                }));
            };
            match index_file(&embedder, &storage, profile_id, file, params, loc, progress).await {
                Ok(n) => chunks_total += n,
                Err(err) => {
                    errors += 1;
                    tracing::warn!(file = %file.display(), error = %err, "RAG: не удалось проиндексировать файл");
                }
            }
        }

        send(RagProgress::Finished {
            files: total,
            chunks: chunks_total,
            errors,
            cancelled: cancel.is_cancelled(),
        });
    });
}

/// Параметры фоновой задачи реиндексации (`/rag rebuild`).
struct RagRebuild {
    embedder: Arc<dyn Embedder>,
    storage: Arc<Storage>,
    profile_id: Uuid,
    params: ChunkParams,
    cancel: CancellationToken,
    /// Язык интерфейса (ось B) — для сообщений о прогрессе/ошибках, видимых человеку.
    loc: &'static Locale,
    evt_tx: tokio::sync::mpsc::UnboundedSender<AppEvent>,
}

/// Запускает фоновую реиндексацию базы знаний профиля (spec §9.3). Собирает
/// исходники (сохранённый текст, иначе — чтение файла по пути), при необходимости
/// сбрасывает таблицу векторов (смена размерности embedding-модели), затем
/// перечанковывает/переэмбеддивает каждый источник текущими параметрами.
fn spawn_rag_rebuild(task: RagRebuild) {
    let RagRebuild {
        embedder,
        storage,
        profile_id,
        params,
        cancel,
        loc,
        evt_tx,
    } = task;

    tokio::spawn(async move {
        let send = |p: RagProgress| {
            let _ = evt_tx.send(AppEvent::RagProgress(p));
        };

        // 1. Собираем источники: что сейчас в базе + их сохранённый текст.
        let infos = match storage.db().rag_list_sources(profile_id) {
            Ok(v) => v,
            Err(err) => {
                send(RagProgress::Failed(
                    loc.tf("ui.err.rag_read_kb", &[("err", &err.to_string())]),
                ));
                return;
            }
        };
        if infos.is_empty() {
            send(RagProgress::Failed(loc.t("ui.err.rag_kb_empty").into()));
            return;
        }
        let stored: HashMap<String, String> = match storage.db().rag_stored_sources(profile_id) {
            Ok(v) => v.into_iter().map(|s| (s.source, s.content)).collect(),
            Err(err) => {
                send(RagProgress::Failed(
                    loc.tf("ui.err.rag_read_sources", &[("err", &err.to_string())]),
                ));
                return;
            }
        };

        // 2. Резолвим содержимое каждого источника: сохранённый текст в приоритете,
        //    иначе пробуем прочитать файл по пути (legacy-данные до хранения текста).
        let mut sources: Vec<(String, String)> = Vec::new();
        let mut missing = 0usize;
        for info in &infos {
            if let Some(content) = stored.get(&info.source) {
                sources.push((info.source.clone(), content.clone()));
                continue;
            }
            let path = std::path::Path::new(&info.source);
            if path.is_file()
                && crate::features::rag_ingest::is_supported(path)
                && let Ok(content) = read_source_text(path)
            {
                sources.push((info.source.clone(), content));
            } else {
                // Исходник не сохранён и файла нет — этот источник восстановить нельзя.
                missing += 1;
                tracing::warn!(source = %info.source, "RAG rebuild: исходник недоступен, источник пропущен");
            }
        }
        if sources.is_empty() {
            send(RagProgress::Failed(
                loc.t("ui.err.rag_no_source_text").into(),
            ));
            return;
        }

        // 3. Предпроверка эмбеддера и определение новой размерности.
        let new_dim = match embedder.embed(vec!["ping".into()]).await {
            Ok(v) => v.first().map(|e| e.len()).unwrap_or(0),
            Err(err) => {
                send(RagProgress::Failed(loc.tf(
                    "ui.err.rag_embedder_unavailable",
                    &[("err", &err.to_string())],
                )));
                return;
            }
        };
        if new_dim == 0 {
            send(RagProgress::Failed(loc.t("ui.err.rag_empty_vector").into()));
            return;
        }

        // 4. Если размерность сменилась (другая embedding-модель), таблицу векторов
        //    нужно пересоздать — но она общая на всю БД. Если у других профилей есть
        //    документы, отказываем (не затираем чужие данные); иначе сбрасываем.
        let current_dim = storage.db().rag_dimension().unwrap_or(None);
        let dim_changed = matches!(current_dim, Some(d) if d != new_dim);
        if dim_changed {
            match storage.db().rag_other_profiles_have_docs(profile_id) {
                Ok(true) => {
                    send(RagProgress::Failed(loc.t("ui.err.rag_dim_conflict").into()));
                    return;
                }
                Ok(false) => {}
                Err(err) => {
                    send(RagProgress::Failed(loc.tf(
                        "ui.err.rag_profiles_check",
                        &[("err", &err.to_string())],
                    )));
                    return;
                }
            }
        }

        // 5. Сносим прежние чанки профиля (исходники сохраняем); при смене размерности
        //    дополнительно сбрасываем таблицу векторов (пересоздастся при первой вставке).
        if let Err(err) = storage.db().rag_delete_all_for_profile(profile_id) {
            send(RagProgress::Failed(
                loc.tf("ui.err.rag_clear_chunks", &[("err", &err.to_string())]),
            ));
            return;
        }
        if dim_changed && let Err(err) = storage.db().rag_reset_vectors() {
            send(RagProgress::Failed(loc.tf(
                "ui.err.rag_reset_vectors",
                &[("err", &err.to_string())],
            )));
            return;
        }

        let total = sources.len();
        send(RagProgress::Started { total });

        let mut chunks_total = 0usize;
        let mut errors = missing;
        for (i, (source, content)) in sources.iter().enumerate() {
            if cancel.is_cancelled() {
                break;
            }
            let name = source_display(source);
            // Источник начат (чанкинг впереди) — chunks_total=0 (баннер как раньше).
            send(RagProgress::Indexing {
                index: i + 1,
                total,
                name: name.clone(),
                dir: String::new(),
                chunks_done: 0,
                chunks_total: 0,
            });
            // Прогресс по чанкам (см. spawn_rag_ingest): rebuild зовёт index_source напрямую.
            let progress = |done: usize, tot: usize| {
                let _ = evt_tx.send(AppEvent::RagProgress(RagProgress::Indexing {
                    index: i + 1,
                    total,
                    name: name.clone(),
                    dir: String::new(),
                    chunks_done: done,
                    chunks_total: tot,
                }));
            };
            match index_source(
                &embedder, &storage, profile_id, source, content, params, loc, progress,
            )
            .await
            {
                Ok(n) => chunks_total += n,
                Err(err) => {
                    errors += 1;
                    tracing::warn!(source = %source, error = %err, "RAG rebuild: не удалось переиндексировать источник");
                }
            }
        }

        send(RagProgress::Finished {
            files: total,
            chunks: chunks_total,
            errors,
            cancelled: cancel.is_cancelled(),
        });
    });
}

/// Читает исходник для индексации: для `.html`/`.htm` извлекает читаемый текст
/// (переиспользует web::extract_readable — отбрасывает nav/header/footer/aside/
/// скрипты), для прочего — как есть. RAG чанкует источник целиком, поэтому
/// извлечение без усечения (usize::MAX). BOM снимает read_text.
fn read_source_text(path: &std::path::Path) -> std::io::Result<String> {
    let raw = crate::features::rag_ingest::read_text(path)?;
    if crate::features::rag_ingest::is_html(path) {
        Ok(crate::features::tools::web::extract_readable(
            &raw,
            usize::MAX,
        ))
    } else {
        Ok(raw)
    }
}

/// Индексирует один файл: читает текст, чанкует, эмбеддит и пишет документы в
/// хранилище (изоляция по `profile_id`). Возвращает число записанных чанков.
async fn index_file(
    embedder: &Arc<dyn Embedder>,
    storage: &Arc<Storage>,
    profile_id: Uuid,
    path: &std::path::Path,
    params: ChunkParams,
    loc: &'static Locale,
    progress: impl FnMut(usize, usize),
) -> anyhow::Result<usize> {
    let content = read_source_text(path)?;
    // Каноничный ключ источника + идемпотентность: при повторном добавлении того же
    // файла заменяем его прежние чанки, а не плодим дубли (см. [`index_source`]).
    let source = crate::features::rag_ingest::canonical_source(path);
    index_source(
        embedder, storage, profile_id, &source, &content, params, loc, progress,
    )
    .await
}

/// Чанкует/эмбеддит/пишет один источник как единое целое (заменяя его прежние
/// чанки и сохранённый текст). Markdown (`*.md`) чанкуется семантически (по
/// заголовкам), прочее — текстовым чанкером. Возвращает число записанных чанков.
/// Общая логика файловой индексации (`/rag add`) и реиндексации (`/rag rebuild`).
///
/// Эмбеддинг идёт под-батчами по [`EMBED_BATCH_CHUNKS`]: после каждого батча
/// вызывается `progress(chunks_done, chunks_total)`, так что баннер двигается *в
/// ходе* эмбеддинга крупного файла. Файл с ≤16 чанками — по-прежнему один запрос.
// Аргументы когезивны (зависимости индексации + колбэк прогресса) и передаются
// позиционно из двух вызывающих; выделять пучок ради одного лишнего параметра —
// лишний churn.
#[allow(clippy::too_many_arguments)]
async fn index_source(
    embedder: &Arc<dyn Embedder>,
    storage: &Arc<Storage>,
    profile_id: Uuid,
    source: &str,
    content: &str,
    params: ChunkParams,
    loc: &'static Locale,
    mut progress: impl FnMut(usize, usize),
) -> anyhow::Result<usize> {
    let chunks = if is_markdown_source(source) {
        crate::features::tools::rag::chunk_markdown(content, params)
    } else {
        crate::features::tools::rag::chunk_text(content, params)
    };
    // Даже у пустого источника обновляем сохранённый текст и сносим прежние чанки —
    // иначе устаревшие чанки остались бы в базе после реиндексации.
    storage.db().rag_delete_by_source(profile_id, source)?;
    storage
        .db()
        .rag_source_upsert(profile_id, source, content, chrono::Utc::now())?;
    if chunks.is_empty() {
        progress(0, 0);
        return Ok(0);
    }
    let total = chunks.len();
    progress(0, total);
    // Эмбеддим и пишем под-батчами: каждый запрос ограничен EMBED_BATCH_CHUNKS, и
    // прогресс двигается по мере готовности батчей (у крупных файлов — плавно).
    let mut done = 0usize;
    for batch in chunks.chunks(EMBED_BATCH_CHUNKS) {
        let embeddings = embedder.embed(batch.to_vec()).await?;
        if embeddings.len() != batch.len() {
            anyhow::bail!("{}", loc.t("ui.err.rag_wrong_vector_count"));
        }
        for (chunk, embedding) in batch.iter().zip(embeddings) {
            let doc = crate::entities::rag::RagDocument::new(profile_id, source, chunk, embedding);
            storage.db().rag_insert(&doc)?;
        }
        done += batch.len();
        progress(done, total);
    }
    Ok(total)
}

/// Источник — markdown (чанкуется по заголовкам)? Решаем по расширению `*.md`.
fn is_markdown_source(source: &str) -> bool {
    std::path::Path::new(source)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("md"))
}

/// Короткое имя источника для индикации прогресса (имя файла, иначе сам источник).
fn source_display(source: &str) -> String {
    std::path::Path::new(source)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| source.to_string())
}

/// Имя файла и его родительская папка (для индикации прогресса индексации).
fn display_parts(path: &std::path::Path) -> (String, String) {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let dir = path
        .parent()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    (name, dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_source_text_extracts_html_and_passes_through_plain() {
        let dir = tempfile::tempdir().unwrap();

        // HTML: boilerplate (nav/header/script) отбрасывается, извлекается абзац статьи.
        let html = dir.path().join("page.html");
        std::fs::write(
            &html,
            "<html><head><script>var secret = 'скриптовый мусор';</script></head>\
             <body><nav>навигационное меню сайта здесь</nav>\
             <header>шапка страницы с логотипом</header>\
             <article><p>Осмысленный абзац содержимого статьи, достаточно длинный, \
             чтобы пройти порог отсева коротких фрагментов извлечения.</p></article>\
             </body></html>",
        )
        .unwrap();
        let extracted = read_source_text(&html).unwrap();
        assert!(
            extracted.contains("Осмысленный абзац содержимого статьи"),
            "извлечённый текст должен содержать абзац статьи: {extracted:?}"
        );
        assert!(
            !extracted.contains("навигационное меню"),
            "nav не должен попадать в извлечённый текст: {extracted:?}"
        );
        assert!(
            !extracted.contains("шапка страницы"),
            "header не должен попадать в извлечённый текст: {extracted:?}"
        );
        assert!(
            !extracted.contains("скриптовый мусор"),
            "script не должен попадать в извлечённый текст: {extracted:?}"
        );

        // Не-HTML: содержимое возвращается дословно (BOM снимает read_text).
        let txt = dir.path().join("note.txt");
        let body = "<p>это не HTML</p>\nобычный текст с угловыми скобками";
        std::fs::write(&txt, body).unwrap();
        assert_eq!(read_source_text(&txt).unwrap(), body);
    }

    /// Файловое хранилище на tempdir + детерминированный эмбеддер для тестов
    /// индексации. Guard каталога возвращается — держать его живым на время теста
    /// (Storage хранит открытое соединение с файлом БД внутри каталога).
    fn test_deps() -> (tempfile::TempDir, Arc<Storage>, Arc<dyn Embedder>) {
        let dir = tempfile::tempdir().unwrap();
        let storage =
            Arc::new(Storage::open(crate::shared::paths::Paths::with_root(dir.path())).unwrap());
        let embedder: Arc<dyn Embedder> = Arc::new(crate::shared::api::mock::MockEmbedder::new(16));
        (dir, storage, embedder)
    }

    #[tokio::test]
    async fn index_source_reports_chunk_progress_in_subbatches() {
        let (_dir, storage, embedder) = test_deps();
        let profile_id = Uuid::new_v4();
        // Мелкий целевой размер → много чанков (> EMBED_BATCH_CHUNKS), чтобы эмбеддинг
        // шёл несколькими под-батчами и прогресс двигался по ходу.
        let params = ChunkParams::from_settings(&crate::shared::config::RagSettings {
            chunk_target_chars: 60,
            chunk_overlap_chars: 10,
            chunk_max_chars: 120,
        });
        let content = "Короткое предложение для проверки чанкинга номер. ".repeat(40);

        let mut ticks: Vec<(usize, usize)> = Vec::new();
        let n = index_source(
            &embedder,
            &storage,
            profile_id,
            "kb.txt",
            &content,
            params,
            crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru),
            |done, total| ticks.push((done, total)),
        )
        .await
        .unwrap();

        assert!(
            n > EMBED_BATCH_CHUNKS,
            "тест должен произвести > {EMBED_BATCH_CHUNKS} чанков, получено {n}"
        );
        // Первый тик — (0, N), последний — (N, N).
        assert_eq!(
            ticks.first(),
            Some(&(0, n)),
            "первый тик — (0, N): {ticks:?}"
        );
        assert_eq!(
            ticks.last(),
            Some(&(n, n)),
            "последний тик — (N, N): {ticks:?}"
        );
        // `done` монотонно не убывает, `total` постоянен и равен возвращённому N.
        for w in ticks.windows(2) {
            assert!(w[1].0 >= w[0].0, "done не убывает: {ticks:?}");
            assert_eq!(w[0].1, n, "total постоянен и равен N");
        }
        // Под-батчинг ничего не потерял: все N чанков записаны и находятся поиском
        // (rag_search — тот же примитив БД, что и инструмент RagSearch).
        assert_eq!(storage.db().rag_count(profile_id).unwrap(), n);
        let mut q = embedder.embed(vec!["предложение".into()]).await.unwrap();
        let query = q.remove(0);
        let hits = storage.db().rag_search(profile_id, &query, 5).unwrap();
        assert!(!hits.is_empty(), "поиск находит записанные чанки");
    }

    #[tokio::test]
    async fn index_source_empty_content_single_zero_tick() {
        let (_dir, storage, embedder) = test_deps();
        let profile_id = Uuid::new_v4();
        let mut ticks: Vec<(usize, usize)> = Vec::new();
        let n = index_source(
            &embedder,
            &storage,
            profile_id,
            "empty.txt",
            "",
            ChunkParams::default(),
            crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru),
            |done, total| ticks.push((done, total)),
        )
        .await
        .unwrap();
        assert_eq!(n, 0, "у пустого источника нет чанков");
        assert_eq!(ticks, vec![(0, 0)], "ровно один тик (0,0): {ticks:?}");
    }
}
