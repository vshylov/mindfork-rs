//! Экран чата — баннер прогресса индексации RAG. Часть модуля [`super`]; разбито из
//! монолита chat.rs (см. docs/history/refactoring-god-objects.md, этап 2).

use super::*;

impl ChatScreen {
    /// Обновляет индикатор фоновой индексации RAG (`/rag add`). Старт/прогресс
    /// показывают баннер со спиннером; завершение/ошибка гасят его и оставляют
    /// итоговую заметку в ленте. См. spec §9.3.
    pub fn set_rag_progress(&mut self, progress: RagProgress) {
        match progress {
            RagProgress::Started { total } => {
                self.rag = Some(RagBanner {
                    text: format!("найдено файлов: {total}, начинаю индексацию…"),
                    tick: 0,
                });
            }
            RagProgress::Indexing {
                index,
                total,
                name,
                dir,
            } => {
                let location = if dir.is_empty() {
                    String::new()
                } else {
                    format!(" из {dir}")
                };
                let text = format!("индексация {name}{location} ({index}/{total})");
                match &mut self.rag {
                    Some(banner) => banner.text = text,
                    None => self.rag = Some(RagBanner { text, tick: 0 }),
                }
            }
            RagProgress::Finished {
                files,
                chunks,
                errors,
                cancelled,
            } => {
                self.rag = None;
                let mut msg = if cancelled {
                    format!("RAG: индексация прервана — фрагментов добавлено: {chunks}")
                } else {
                    format!("RAG: индексация завершена — файлов: {files}, фрагментов: {chunks}")
                };
                if errors > 0 {
                    msg.push_str(&format!(", с ошибками: {errors}"));
                }
                self.push_note(&msg);
            }
            RagProgress::Removed { chunks } => {
                let msg = if chunks == 0 {
                    "RAG: по указанному пути ничего не найдено в базе".to_string()
                } else {
                    format!("RAG: удалено фрагментов: {chunks}")
                };
                self.push_note(&msg);
            }
            RagProgress::Listed { sources } => {
                self.push_note(&format_rag_sources(&sources));
            }
            RagProgress::Failed(err) => {
                self.rag = None;
                self.push_error(&format!("RAG: {err}"));
            }
        }
    }

    /// Идёт ли фоновая индексация RAG (петля перерисовывает кадры для анимации
    /// спиннера, пока это `true`).
    pub fn is_rag_active(&self) -> bool {
        self.rag.is_some()
    }
}

pub(super) fn format_rag_sources(sources: &[crate::entities::rag::RagSourceInfo]) -> String {
    if sources.is_empty() {
        return "RAG: база знаний пуста".to_string();
    }
    let total: usize = sources.iter().map(|s| s.chunks).sum();
    let mut out = format!("RAG: источников: {}, фрагментов: {total}", sources.len());
    for s in sources {
        out.push_str(&format!(
            "\n• {} — {} фрагм. ({})",
            s.source,
            s.chunks,
            s.created_at
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%d")
        ));
    }
    out
}
