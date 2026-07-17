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
                    text: self
                        .loc
                        .tf("ui.rag.started", &[("total", &total.to_string())]),
                    tick: 0,
                });
            }
            RagProgress::Indexing {
                index,
                total,
                name,
                dir,
                chunks_done,
                chunks_total,
            } => {
                let location = if dir.is_empty() {
                    String::new()
                } else {
                    self.loc.tf("ui.rag.from", &[("dir", &dir)])
                };
                let mut text = self.loc.tf(
                    "ui.rag.indexing",
                    &[
                        ("name", &name),
                        ("location", &location),
                        ("index", &index.to_string()),
                        ("total", &total.to_string()),
                    ],
                );
                // Прогресс по чанкам показываем, только когда файл уже чанкован
                // (chunks_total>0). Иначе (файл только начат) — вид как раньше.
                if chunks_total > 0 {
                    text.push_str(&self.loc.tf(
                        "ui.rag.chunks",
                        &[
                            ("done", &chunks_done.to_string()),
                            ("total", &chunks_total.to_string()),
                        ],
                    ));
                }
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
                    self.loc.tf(
                        "ui.rag.finished_cancelled",
                        &[("chunks", &chunks.to_string())],
                    )
                } else {
                    self.loc.tf(
                        "ui.rag.finished",
                        &[
                            ("files", &files.to_string()),
                            ("chunks", &chunks.to_string()),
                        ],
                    )
                };
                if errors > 0 {
                    msg.push_str(
                        &self
                            .loc
                            .tf("ui.rag.errors_suffix", &[("errors", &errors.to_string())]),
                    );
                }
                self.push_note(&msg);
            }
            RagProgress::Removed { chunks } => {
                let msg = if chunks == 0 {
                    self.loc.t("ui.rag.removed_none").to_string()
                } else {
                    self.loc
                        .tf("ui.rag.removed", &[("chunks", &chunks.to_string())])
                };
                self.push_note(&msg);
            }
            RagProgress::Listed { sources } => {
                self.push_note(&format_rag_sources(&sources, self.loc));
            }
            RagProgress::Failed(err) => {
                self.rag = None;
                self.push_error(&self.loc.tf("ui.rag.failed", &[("err", &err)]));
            }
        }
    }

    /// Идёт ли фоновая индексация RAG (петля перерисовывает кадры для анимации
    /// спиннера, пока это `true`).
    pub fn is_rag_active(&self) -> bool {
        self.rag.is_some()
    }
}

pub(super) fn format_rag_sources(
    sources: &[crate::entities::rag::RagSourceInfo],
    loc: &'static Locale,
) -> String {
    if sources.is_empty() {
        return loc.t("ui.rag.list_empty").to_string();
    }
    let total: usize = sources.iter().map(|s| s.chunks).sum();
    let mut out = loc.tf(
        "ui.rag.list_header",
        &[
            ("n", &sources.len().to_string()),
            ("total", &total.to_string()),
        ],
    );
    for s in sources {
        let date = s
            .created_at
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d")
            .to_string();
        out.push_str(&loc.tf(
            "ui.rag.list_item",
            &[
                ("source", &s.source),
                ("chunks", &s.chunks.to_string()),
                ("date", &date),
            ],
        ));
    }
    out
}
