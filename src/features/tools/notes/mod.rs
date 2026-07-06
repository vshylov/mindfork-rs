//! Инструменты заметок: `note_save`, `note_recall`. Память ассистента о
//! пользователе/контексте, **изолированная по `profile_id`** (spec §9.3, §9.5).

use anyhow::Result;
use uuid::Uuid;

use crate::entities::note::Note;
use crate::entities::profile::ToolId;

use super::{Tool, ToolContext, ToolOutcome};

/// Имя инструмента припоминания заметок (нужно рефлексии для кросс-органных связей,
/// Ярус 3 — id пользовательских заметок).
pub const NOTE_RECALL_ID: &str = "note_recall";
/// Имя инструмента ревизии заметки (DB-only, гейтится набором профиля).
pub const NOTE_REVISE_ID: &str = "note_revise";
/// Граф связей и ревизионная история (Ярус 2, DB-only, гейтятся набором профиля).
pub const NOTE_LINK_ID: &str = "note_link";
pub const NOTE_NEIGHBORS_ID: &str = "note_neighbors";
pub const NOTE_SUPERSEDE_ID: &str = "note_supersede";
pub const NOTE_MERGE_ID: &str = "note_merge";
pub const CONSOLIDATE_NOTES_ID: &str = "consolidate_notes";
/// Связь заметки с RAG-источником (Ярус 3, Путь 3 — связывание органов памяти).
pub const NOTE_CITE_SOURCE_ID: &str = "note_cite_source";

/// Порог косинусной близости, при котором две заметки считаются возможным дублем
/// (для обзора консолидации). Подобран эмпирически — пары выше стоит рассмотреть.
const CONSOLIDATE_SIMILARITY: f32 = 0.85;
/// Сколько элементов максимум показывать в каждой секции обзора консолидации.
const CONSOLIDATE_LIST_CAP: usize = 8;

/// Типы связей между заметками (направленные). Зеркалят схему инструмента `note_link`.
const RELATIONS: [&str; 4] = ["supports", "contradicts", "refines", "relates"];

/// Сколько заметок отдаёт `note_recall` по умолчанию (если лимит не задан).
const DEFAULT_RECALL: usize = 5;

/// Размер батча при бэкфилле эмбеддингов «старых» заметок.
const NOTE_BACKFILL_BATCH: usize = 32;

/// Сколько связанных заметок максимум подмешивать в `note_recall` (spreading activation).
const RELATED_IN_RECALL: usize = 5;

/// Зарезервированный тег «заметок о себе»: нарратив «модели себя» переехал в обычные
/// заметки (см. docs/history/narrative-as-notes.md, Ярус 1). Self-заметки делят таблицы,
/// эмбеддинги, граф и консолидацию с обычными, но **скрыты** из пользовательского
/// `note_recall`, ворот `note_save` и обзора консолидации фильтром по этому тегу —
/// память о себе ≠ память о собеседнике, смешение выдачи рискованно. Лидирующий `@`
/// не встречается в естественных тегах; коллизия редка и безобидна (такая заметка
/// просто станет считаться инсайтом). Читаются self-заметки отдельным путём
/// (`self_notes_recent`).
pub const SELF_NOTE_TAG: &str = "@self";

/// Несёт ли заметка зарезервированный тег [`SELF_NOTE_TAG`] («заметка о себе»).
pub(crate) fn is_self_note(note: &Note) -> bool {
    note.tags.iter().any(|t| t == SELF_NOTE_TAG)
}

/// Парсит uuid из строкового поля аргументов с понятной ошибкой.
fn parse_id(args: &serde_json::Value, key: &str) -> Result<Uuid> {
    let raw = args
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .trim();
    Uuid::parse_str(raw).map_err(|_| anyhow::anyhow!("некорректный id ({key}): {raw}"))
}

/// Косинусная близость двух векторов (0 при разной длине/нулевой норме).
/// `pub(crate)` — переиспользуется воротами почти-дублей черт `user_model`
/// (`self_model::UpdateUserModel`), где вектора черт эмбеддятся на лету.
pub(crate) fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na * nb)
}

/// Усечение строки по символам (для компактного обзора).
fn clip(s: &str, n: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= n {
        return s.to_string();
    }
    let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Извлекает массив строковых тегов из аргументов (пустой, если нет).
fn parse_tags(args: &serde_json::Value) -> Vec<String> {
    args.get("tags")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|t| t.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

// ---------- подмодули (разбор god-object: docs/refactoring-god-objects.md, этап 4) ----------

mod cite;
mod edit;
mod graph;
mod overview;
mod recall;
mod save;
mod self_notes;

// Реэкспорт всей внешней поверхности `notes::*` (инструменты + pub(crate)-хелперы),
// чтобы внешние `use crate::features::tools::notes::X` не менялись.
pub(crate) use self::{cite::*, edit::*, graph::*, overview::*, recall::*, save::*, self_notes::*};

#[cfg(test)]
mod tests;
