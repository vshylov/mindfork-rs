//! Инструменты доступа к локальным файлам (spec §9.3, §13.2): `fs_read`,
//! `fs_write`, `fs_list`. Под глобальным выключателем `tools.fs_enabled` (по
//! умолчанию выключены — инструмент может прочитать/перезаписать любой файл).
//!
//! Опциональная «песочница» `tools.fs_root`: если задана, все пути обязаны лежать
//! внутри неё (защита от выхода через `..`/абсолютные пути). Если не задана —
//! доступ ко всей файловой системе (под выключателем, как у Python).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::entities::profile::ToolId;

use super::{Tool, ToolContext, ToolOutcome};

/// Имена инструментов (гейтятся выключателем `tools.fs_enabled`).
pub const FS_READ_ID: &str = "fs_read";
pub const FS_WRITE_ID: &str = "fs_write";
pub const FS_LIST_ID: &str = "fs_list";

/// Потолок размера читаемого/возвращаемого текста (символов) — защита контекста.
const MAX_READ_CHARS: usize = 50_000;
/// Потолок числа элементов в листинге каталога.
const MAX_LIST_ENTRIES: usize = 500;

/// Общая «песочница» файловых инструментов: опциональный корень-ограничитель.
#[derive(Clone)]
struct FsRoot {
    /// Каноничный каталог-ограничитель (`None` → без ограничения).
    root: Option<PathBuf>,
}

impl FsRoot {
    fn new(root: Option<String>) -> Self {
        Self {
            root: root
                .filter(|s| !s.trim().is_empty())
                .map(|s| PathBuf::from(s.trim())),
        }
    }

    /// Резолвит путь из аргумента и проверяет, что он внутри песочницы (если задана).
    /// Для существующих путей сравнение идёт по каноничной форме; для ещё не
    /// существующих (запись нового файла) канонизируется родительский каталог.
    fn resolve(&self, raw: &str) -> Result<PathBuf> {
        let raw = raw.trim();
        if raw.is_empty() {
            anyhow::bail!("пустой путь");
        }
        let requested = PathBuf::from(raw);
        let Some(root) = &self.root else {
            return Ok(requested);
        };
        let root = root
            .canonicalize()
            .with_context(|| format!("каталог-песочница недоступен: {}", root.display()))?;
        // Абсолютный путь берётся как есть, относительный — от корня песочницы.
        let candidate = if requested.is_absolute() {
            requested
        } else {
            root.join(&requested)
        };
        // Каноничная форма самого пути (если существует) либо его родителя + имя.
        let canonical = match candidate.canonicalize() {
            Ok(c) => c,
            Err(_) => {
                let parent = candidate
                    .parent()
                    .ok_or_else(|| anyhow::anyhow!("у пути нет родительского каталога"))?;
                let parent = parent.canonicalize().with_context(|| {
                    format!("родительский каталог недоступен: {}", parent.display())
                })?;
                let name = candidate
                    .file_name()
                    .ok_or_else(|| anyhow::anyhow!("у пути нет имени файла"))?;
                parent.join(name)
            }
        };
        if !canonical.starts_with(&root) {
            anyhow::bail!(
                "путь вне разрешённого каталога ({}). Доступ ограничен песочницей.",
                root.display()
            );
        }
        Ok(canonical)
    }
}

/// Достаёт строковый аргумент `path`.
fn arg_path(args: &serde_json::Value) -> Result<String> {
    args.get("path")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow::anyhow!("ожидается непустое поле path"))
}

/// Усекает строку до `max` символов (по границе символа) с пометкой.
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max).collect();
    format!("{cut}\n…(содержимое обрезано, показаны первые {max} символов)")
}

/// `fs_read` — читает текстовый файл и возвращает его содержимое.
pub struct FsRead {
    fs: FsRoot,
}

impl FsRead {
    pub fn new(root: Option<String>) -> Self {
        Self {
            fs: FsRoot::new(root),
        }
    }
}

#[async_trait::async_trait]
impl Tool for FsRead {
    fn id(&self) -> ToolId {
        FS_READ_ID.into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Files
    }
    fn ui_label(&self) -> &'static str {
        "прочитать файл"
    }
    fn gate(&self) -> Option<crate::features::tools::meta::ToolGate> {
        Some(crate::features::tools::meta::ToolGate::Fs)
    }
    fn description(&self, _loc: &crate::shared::i18n::Locale) -> String {
        "Прочитать текстовый файл и вернуть его содержимое (с лимитом на размер).".into()
    }
    fn parameters(&self, _loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {"path": {"type": "string", "description": "Путь к файлу"}},
            "required": ["path"]
        })
    }
    async fn invoke(&self, _ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let path = self.fs.resolve(&arg_path(&args)?)?;
        let bytes = match tokio::fs::read(&path).await {
            Ok(b) => b,
            Err(err) => {
                return Ok(ToolOutcome::text(format!(
                    "Не удалось прочитать {}: {err}",
                    path.display()
                )));
            }
        };
        // Читаем как UTF-8 (с заменой неверных байтов) — бинарные файлы читать нечем.
        let text = String::from_utf8_lossy(&bytes);
        Ok(ToolOutcome::text(truncate_chars(&text, MAX_READ_CHARS)))
    }
}

/// `fs_write` — записывает (или дописывает) текст в файл.
pub struct FsWrite {
    fs: FsRoot,
}

impl FsWrite {
    pub fn new(root: Option<String>) -> Self {
        Self {
            fs: FsRoot::new(root),
        }
    }
}

#[async_trait::async_trait]
impl Tool for FsWrite {
    fn id(&self) -> ToolId {
        FS_WRITE_ID.into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Files
    }
    fn ui_label(&self) -> &'static str {
        "записать файл"
    }
    fn gate(&self) -> Option<crate::features::tools::meta::ToolGate> {
        Some(crate::features::tools::meta::ToolGate::Fs)
    }
    fn description(&self, _loc: &crate::shared::i18n::Locale) -> String {
        "Записать текст в файл (перезаписывает существующий). Передай append=true, \
         чтобы дописать в конец."
            .into()
    }
    fn parameters(&self, _loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Путь к файлу"},
                "content": {"type": "string", "description": "Содержимое для записи"},
                "append": {"type": "boolean", "description": "Дописать в конец (по умолчанию false)"}
            },
            "required": ["path", "content"]
        })
    }
    async fn invoke(&self, _ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let path = self.fs.resolve(&arg_path(&args)?)?;
        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("ожидается поле content"))?;
        let append = args
            .get("append")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let result = if append {
            append_to(&path, content).await
        } else {
            tokio::fs::write(&path, content.as_bytes()).await
        };
        match result {
            Ok(()) => Ok(ToolOutcome::text(format!(
                "{} {} ({} символов).",
                if append {
                    "Дописано в"
                } else {
                    "Записано в"
                },
                path.display(),
                content.chars().count()
            ))),
            Err(err) => Ok(ToolOutcome::text(format!(
                "Не удалось записать {}: {err}",
                path.display()
            ))),
        }
    }
}

/// Дописывает в конец файла (создаёт, если нет).
async fn append_to(path: &Path, content: &str) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await?;
    file.write_all(content.as_bytes()).await
}

/// `fs_list` — перечисляет содержимое каталога.
pub struct FsList {
    fs: FsRoot,
}

impl FsList {
    pub fn new(root: Option<String>) -> Self {
        Self {
            fs: FsRoot::new(root),
        }
    }
}

#[async_trait::async_trait]
impl Tool for FsList {
    fn id(&self) -> ToolId {
        FS_LIST_ID.into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Files
    }
    fn ui_label(&self) -> &'static str {
        "список файлов"
    }
    fn gate(&self) -> Option<crate::features::tools::meta::ToolGate> {
        Some(crate::features::tools::meta::ToolGate::Fs)
    }
    fn description(&self, _loc: &crate::shared::i18n::Locale) -> String {
        "Перечислить содержимое каталога (файлы и подкаталоги).".into()
    }
    fn parameters(&self, _loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {"path": {"type": "string", "description": "Путь к каталогу"}},
            "required": ["path"]
        })
    }
    async fn invoke(&self, _ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let path = self.fs.resolve(&arg_path(&args)?)?;
        let mut rd = match tokio::fs::read_dir(&path).await {
            Ok(rd) => rd,
            Err(err) => {
                return Ok(ToolOutcome::text(format!(
                    "Не удалось открыть каталог {}: {err}",
                    path.display()
                )));
            }
        };
        let mut entries: Vec<String> = Vec::new();
        let mut truncated = false;
        while let Some(entry) = rd.next_entry().await? {
            if entries.len() >= MAX_LIST_ENTRIES {
                truncated = true;
                break;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
            entries.push(if is_dir { format!("{name}/") } else { name });
        }
        entries.sort();
        if entries.is_empty() {
            return Ok(ToolOutcome::text(format!(
                "Каталог {} пуст.",
                path.display()
            )));
        }
        let mut out = format!("Содержимое {} ({}):\n", path.display(), entries.len());
        out.push_str(&entries.join("\n"));
        if truncated {
            out.push_str(&format!("\n…(показаны первые {MAX_LIST_ENTRIES})"));
        }
        Ok(ToolOutcome::text(out))
    }
}

#[cfg(test)]
mod tests {
    use super::super::testkit::ctx_with_storage;
    use super::*;
    use uuid::Uuid;

    #[tokio::test]
    async fn write_then_read_roundtrip() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("note.txt");
        let path = file.to_string_lossy().to_string();

        FsWrite::new(None)
            .invoke(&ctx, serde_json::json!({"path": path, "content": "привет"}))
            .await
            .unwrap();
        let out = FsRead::new(None)
            .invoke(&ctx, serde_json::json!({"path": path}))
            .await
            .unwrap();
        assert_eq!(out.result, "привет");
    }

    #[tokio::test]
    async fn append_adds_to_end() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.txt").to_string_lossy().to_string();
        let w = FsWrite::new(None);
        w.invoke(&ctx, serde_json::json!({"path": path, "content": "a"}))
            .await
            .unwrap();
        w.invoke(
            &ctx,
            serde_json::json!({"path": path, "content": "b", "append": true}),
        )
        .await
        .unwrap();
        let out = FsRead::new(None)
            .invoke(&ctx, serde_json::json!({"path": path}))
            .await
            .unwrap();
        assert_eq!(out.result, "ab");
    }

    #[tokio::test]
    async fn list_shows_entries() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "x").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let path = dir.path().to_string_lossy().to_string();
        let out = FsList::new(None)
            .invoke(&ctx, serde_json::json!({"path": path}))
            .await
            .unwrap();
        assert!(out.result.contains("a.txt"), "got: {}", out.result);
        assert!(out.result.contains("sub/"), "got: {}", out.result);
    }

    #[tokio::test]
    async fn read_missing_file_reports_error_not_panic() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let out = FsRead::new(None)
            .invoke(
                &ctx,
                serde_json::json!({"path": "definitely-not-a-real-file-xyz.txt"}),
            )
            .await
            .unwrap();
        assert!(
            out.result.contains("Не удалось прочитать"),
            "got: {}",
            out.result
        );
    }

    #[tokio::test]
    async fn sandbox_blocks_outside_paths() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("inside.txt"), "ok").unwrap();
        let fs_root = Some(root.path().to_string_lossy().to_string());

        // Внутри песочницы — читается.
        let inside = FsRead::new(fs_root.clone())
            .invoke(&ctx, serde_json::json!({"path": "inside.txt"}))
            .await
            .unwrap();
        assert_eq!(inside.result, "ok");

        // Выход через `..` — отклоняется (ошибка инструмента).
        let escape = FsRead::new(fs_root)
            .invoke(&ctx, serde_json::json!({"path": "../../etc/passwd"}))
            .await;
        assert!(escape.is_err(), "выход из песочницы должен быть отклонён");
    }

    #[tokio::test]
    async fn sandbox_allows_writing_new_file_inside() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let root = tempfile::tempdir().unwrap();
        let fs_root = Some(root.path().to_string_lossy().to_string());
        let out = FsWrite::new(fs_root)
            .invoke(
                &ctx,
                serde_json::json!({"path": "new.txt", "content": "data"}),
            )
            .await
            .unwrap();
        assert!(out.result.contains("Записано"), "got: {}", out.result);
        assert_eq!(
            std::fs::read_to_string(root.path().join("new.txt")).unwrap(),
            "data"
        );
    }

    #[tokio::test]
    async fn rejects_empty_path() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        assert!(
            FsRead::new(None)
                .invoke(&ctx, serde_json::json!({"path": "  "}))
                .await
                .is_err()
        );
    }
}
