//! Загрузка/удаление файлов в базе знаний (RAG) командами `/rag add|remove`
//! (spec §9.3). Индексация — отменяемая фоновая задача с прогрессом; удаление —
//! быстрая операция БД на месте. Всё с изоляцией по `profile_id`.

use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::app::events::{AppEvent, RagProgress};
use crate::shared::api::Embedder;
use crate::shared::storage::Storage;

use super::Orchestrator;

impl Orchestrator {
    /// Запускает фоновую индексацию файла/директории в RAG активного профиля
    /// (команда `/rag add`, spec §9.3). Сканирование, чтение, эмбеддинг и запись
    /// идут в отдельной задаче; прогресс — событиями [`RagProgress`]. Предыдущая
    /// незавершённая индексация отменяется (одна за раз).
    pub(super) fn handle_rag_add(&mut self, path: String, recursive: bool) {
        let path = path.trim().to_string();
        if path.is_empty() {
            return;
        }
        // Профиль берём из активного чата (RAG изолирован по `profile_id`, §9.5).
        let Some(profile_id) = self
            .active_id
            .and_then(|id| self.chats.iter().find(|c| c.id == id))
            .map(|c| c.profile_id)
        else {
            let _ = self.evt_tx.send(AppEvent::RagProgress(RagProgress::Failed(
                "нет активного чата".into(),
            )));
            return;
        };

        // Отменяем предыдущую индексацию, если шла, и заводим новый токен.
        if let Some(token) = self.rag_cancel.take() {
            token.cancel();
        }
        let cancel = CancellationToken::new();
        self.rag_cancel = Some(cancel.clone());

        spawn_rag_ingest(RagIngest {
            embedder: self.embedder.clone(),
            storage: self.storage.clone(),
            profile_id,
            root: std::path::PathBuf::from(path),
            recursive,
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
        let Some(profile_id) = self
            .active_id
            .and_then(|id| self.chats.iter().find(|c| c.id == id))
            .map(|c| c.profile_id)
        else {
            let _ = self.evt_tx.send(AppEvent::RagProgress(RagProgress::Failed(
                "нет активного чата".into(),
            )));
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
}

/// Параметры фоновой задачи индексации файлов в RAG (`/rag add`).
struct RagIngest {
    embedder: Arc<dyn Embedder>,
    storage: Arc<Storage>,
    profile_id: Uuid,
    root: std::path::PathBuf,
    recursive: bool,
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
            match index_file(&embedder, &storage, profile_id, file).await {
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

/// Индексирует один файл: читает текст, чанкует, эмбеддит и пишет документы в
/// хранилище (изоляция по `profile_id`). Возвращает число записанных чанков.
async fn index_file(
    embedder: &Arc<dyn Embedder>,
    storage: &Arc<Storage>,
    profile_id: Uuid,
    path: &std::path::Path,
) -> anyhow::Result<usize> {
    let content = crate::features::rag_ingest::read_text(path)?;
    // Markdown чанкуем семантически (по заголовкам), прочее — текстовым чанкером.
    let is_markdown = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("md"));
    let chunks = if is_markdown {
        crate::features::tools::rag::chunk_markdown(&content)
    } else {
        crate::features::tools::rag::chunk_text(&content)
    };
    if chunks.is_empty() {
        return Ok(0);
    }
    let embeddings = embedder.embed(chunks.clone()).await?;
    if embeddings.len() != chunks.len() {
        anyhow::bail!("эмбеддер вернул неверное число векторов");
    }
    // Каноничный ключ источника + идемпотентность: при повторном добавлении того же
    // файла заменяем его прежние чанки, а не плодим дубликаты.
    let source = crate::features::rag_ingest::canonical_source(path);
    storage.db().rag_delete_by_source(profile_id, &source)?;
    for (chunk, embedding) in chunks.iter().zip(embeddings) {
        let doc = crate::entities::rag::RagDocument::new(profile_id, &source, chunk, embedding);
        storage.db().rag_insert(&doc)?;
    }
    Ok(chunks.len())
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
