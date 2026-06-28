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
use crate::shared::storage::Storage;

use super::Orchestrator;

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
            self.fail_rag("нет активного чата");
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
            self.fail_rag("нет активного чата");
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
            Err(err) => RagProgress::Failed(format!("удаление не удалось: {err}")),
        };
        let _ = self.evt_tx.send(AppEvent::RagProgress(progress));
    }

    /// Показывает источники базы знаний активного профиля (команда `/rag list`):
    /// по каждому источнику — число чанков и дата. Быстрая операция БД на месте.
    pub(super) fn handle_rag_list(&mut self) {
        let Some(profile_id) = self.active_profile_id() else {
            self.fail_rag("нет активного чата");
            return;
        };
        let progress = match self.storage.db().rag_list_sources(profile_id) {
            Ok(sources) => RagProgress::Listed { sources },
            Err(err) => RagProgress::Failed(format!("не удалось прочитать базу знаний: {err}")),
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
            self.fail_rag("нет активного чата");
            return;
        };
        let cancel = self.reset_rag_cancel();
        spawn_rag_rebuild(RagRebuild {
            embedder: self.engines.embedder(),
            storage: self.storage.clone(),
            profile_id,
            params: self.chunk_params(),
            cancel,
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
        evt_tx,
    } = task;

    tokio::spawn(async move {
        let send = |p: RagProgress| {
            let _ = evt_tx.send(AppEvent::RagProgress(p));
        };

        // 1. Сканируем файлы (txt/md). Ошибка пути / пустой результат — понятный отказ.
        let files = match crate::features::rag_ingest::scan(&root, recursive) {
            Ok(files) => files,
            Err(err) => {
                send(RagProgress::Failed(format!("путь недоступен: {err}")));
                return;
            }
        };
        if files.is_empty() {
            send(RagProgress::Failed(
                "не найдено файлов .txt/.md для индексации".into(),
            ));
            return;
        }

        // 2. Предпроверка эмбеддера — быстрый понятный отказ, если RAG не настроен.
        if let Err(err) = embedder.embed(vec!["ping".into()]).await {
            send(RagProgress::Failed(format!("эмбеддер недоступен: {err}")));
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
            send(RagProgress::Indexing {
                index: i + 1,
                total,
                name,
                dir,
            });
            match index_file(&embedder, &storage, profile_id, file, params).await {
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
                send(RagProgress::Failed(format!("чтение базы знаний: {err}")));
                return;
            }
        };
        if infos.is_empty() {
            send(RagProgress::Failed(
                "база знаний пуста — нечего реиндексировать".into(),
            ));
            return;
        }
        let stored: HashMap<String, String> = match storage.db().rag_stored_sources(profile_id) {
            Ok(v) => v.into_iter().map(|s| (s.source, s.content)).collect(),
            Err(err) => {
                send(RagProgress::Failed(format!("чтение исходников: {err}")));
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
                && let Ok(content) = crate::features::rag_ingest::read_text(path)
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
                "не удалось получить исходный текст ни одного источника \
                 (нет сохранённого текста и файлов на диске)"
                    .into(),
            ));
            return;
        }

        // 3. Предпроверка эмбеддера и определение новой размерности.
        let new_dim = match embedder.embed(vec!["ping".into()]).await {
            Ok(v) => v.first().map(|e| e.len()).unwrap_or(0),
            Err(err) => {
                send(RagProgress::Failed(format!("эмбеддер недоступен: {err}")));
                return;
            }
        };
        if new_dim == 0 {
            send(RagProgress::Failed("эмбеддер вернул пустой вектор".into()));
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
                    send(RagProgress::Failed(
                        "сменилась размерность embedding-модели, но базу знаний \
                         используют и другие профили — реиндексируйте их или очистите \
                         сначала их базы"
                            .into(),
                    ));
                    return;
                }
                Ok(false) => {}
                Err(err) => {
                    send(RagProgress::Failed(format!("проверка профилей: {err}")));
                    return;
                }
            }
        }

        // 5. Сносим прежние чанки профиля (исходники сохраняем); при смене размерности
        //    дополнительно сбрасываем таблицу векторов (пересоздастся при первой вставке).
        if let Err(err) = storage.db().rag_delete_all_for_profile(profile_id) {
            send(RagProgress::Failed(format!(
                "очистка прежних чанков: {err}"
            )));
            return;
        }
        if dim_changed && let Err(err) = storage.db().rag_reset_vectors() {
            send(RagProgress::Failed(format!(
                "сброс таблицы векторов: {err}"
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
            send(RagProgress::Indexing {
                index: i + 1,
                total,
                name: source_display(source),
                dir: String::new(),
            });
            match index_source(&embedder, &storage, profile_id, source, content, params).await {
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

/// Индексирует один файл: читает текст, чанкует, эмбеддит и пишет документы в
/// хранилище (изоляция по `profile_id`). Возвращает число записанных чанков.
async fn index_file(
    embedder: &Arc<dyn Embedder>,
    storage: &Arc<Storage>,
    profile_id: Uuid,
    path: &std::path::Path,
    params: ChunkParams,
) -> anyhow::Result<usize> {
    let content = crate::features::rag_ingest::read_text(path)?;
    // Каноничный ключ источника + идемпотентность: при повторном добавлении того же
    // файла заменяем его прежние чанки, а не плодим дубли (см. [`index_source`]).
    let source = crate::features::rag_ingest::canonical_source(path);
    index_source(embedder, storage, profile_id, &source, &content, params).await
}

/// Чанкует/эмбеддит/пишет один источник как единое целое (заменяя его прежние
/// чанки и сохранённый текст). Markdown (`*.md`) чанкуется семантически (по
/// заголовкам), прочее — текстовым чанкером. Возвращает число записанных чанков.
/// Общая логика файловой индексации (`/rag add`) и реиндексации (`/rag rebuild`).
async fn index_source(
    embedder: &Arc<dyn Embedder>,
    storage: &Arc<Storage>,
    profile_id: Uuid,
    source: &str,
    content: &str,
    params: ChunkParams,
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
        return Ok(0);
    }
    let embeddings = embedder.embed(chunks.clone()).await?;
    if embeddings.len() != chunks.len() {
        anyhow::bail!("эмбеддер вернул неверное число векторов");
    }
    for (chunk, embedding) in chunks.iter().zip(embeddings) {
        let doc = crate::entities::rag::RagDocument::new(profile_id, source, chunk, embedding);
        storage.db().rag_insert(&doc)?;
    }
    Ok(chunks.len())
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
