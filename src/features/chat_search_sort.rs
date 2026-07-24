//! Filters and sorts the chat list (pure logic, tested without the UI).
//! See spec §11.2 (substring search, two sort orders).

use crate::entities::chat::ChatSummary;

/// The chat-list sort mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortMode {
    /// By creation date (descending).
    Created,
    /// By last-modified date (descending). The default value.
    #[default]
    Modified,
}

impl SortMode {
    /// Toggles the mode (for the hotkey).
    pub fn toggled(self) -> Self {
        match self {
            SortMode::Created => SortMode::Modified,
            SortMode::Modified => SortMode::Created,
        }
    }
}

/// Filters chats by whether `query` occurs in the title (case-insensitive) and
/// sorts by the corresponding date, descending. An empty `query` filters nothing out.
pub fn filter_and_sort(chats: &[ChatSummary], query: &str, sort: SortMode) -> Vec<ChatSummary> {
    let needle = query.trim().to_lowercase();
    let mut out: Vec<ChatSummary> = chats
        .iter()
        .filter(|c| needle.is_empty() || c.title.to_lowercase().contains(&needle))
        .cloned()
        .collect();
    out.sort_by_key(|c| {
        std::cmp::Reverse(match sort {
            SortMode::Created => c.created_at,
            SortMode::Modified => c.modified_at,
        })
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use uuid::Uuid;

    fn summary(title: &str, created_min: i64, modified_min: i64) -> ChatSummary {
        ChatSummary {
            id: Uuid::new_v4(),
            title: title.to_string(),
            created_at: Utc.timestamp_opt(created_min * 60, 0).unwrap(),
            modified_at: Utc.timestamp_opt(modified_min * 60, 0).unwrap(),
            message_count: 0,
        }
    }

    fn sample() -> Vec<ChatSummary> {
        vec![
            summary("Альфа проект", 1, 30),
            summary("Бета заметки", 2, 10),
            summary("alpha draft", 3, 20),
        ]
    }

    #[test]
    fn filter_is_case_insensitive_and_substring() {
        let chats = sample();
        let r = filter_and_sort(&chats, "alpha", SortMode::Created);
        // The Cyrillic title does not contain the Latin "alpha"; only the
        // "alpha draft" chat matches.
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].title, "alpha draft");

        let r2 = filter_and_sort(&chats, "АЛЬ", SortMode::Created);
        assert_eq!(r2.len(), 1);
        assert_eq!(r2[0].title, "Альфа проект");
    }

    #[test]
    fn empty_query_returns_all() {
        assert_eq!(filter_and_sort(&sample(), "  ", SortMode::Created).len(), 3);
    }

    #[test]
    fn sort_by_created_desc() {
        let r = filter_and_sort(&sample(), "", SortMode::Created);
        assert_eq!(r[0].title, "alpha draft"); // created=3
        assert_eq!(r[2].title, "Альфа проект"); // created=1
    }

    #[test]
    fn sort_by_modified_desc() {
        let r = filter_and_sort(&sample(), "", SortMode::Modified);
        assert_eq!(r[0].title, "Альфа проект"); // modified=30
        assert_eq!(r[2].title, "Бета заметки"); // modified=10
    }

    #[test]
    fn toggle_cycles_modes() {
        assert_eq!(SortMode::Modified.toggled(), SortMode::Created);
        assert_eq!(SortMode::Created.toggled(), SortMode::Modified);
    }
}
