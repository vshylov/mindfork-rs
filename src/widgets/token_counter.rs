//! The token counter: the conversation and the reply as one number, and its
//! two notes — the reasoning tokens in the reply and the share of the
//! conversation the provider read from its cache. Drawn at the right corner of
//! the feed's bottom border (spec §11.3); where a short window takes that
//! border away, in the status bar's pill instead (spec §11.1, §11.1.1). One
//! type for both places, so the two cannot word the same turn differently.

use ratatui::style::Style;
use ratatui::text::Span;

use crate::shared::config::InterfaceSettings;
use crate::shared::i18n::Locale;
use crate::shared::theme::Palette;

/// The counter's figures for this frame and which of its notes are wanted.
/// The screen assembles it ([`ChatScreen::token_counter`](crate::screens::chat::ChatScreen)).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TokenCounter {
    /// A turn is running: the counter grows live and is drawn bright; after
    /// it, the last turn's final figures stay, muted.
    pub generating: bool,
    /// Reply tokens (live).
    pub tokens: u64,
    /// Conversation (prompt) tokens; `None` — unknown.
    pub context: Option<u64>,
    /// `context` is the exact figure from the server's `usage`; otherwise it
    /// is the client's estimate, marked with `~`.
    pub context_exact: bool,
    /// Reasoning tokens ("thoughts") in the reply (included in `tokens`); `0`
    /// — none.
    pub reasoning: u32,
    /// How much of `context` the provider read from its cache; `None` — no
    /// figure for this request (docs/research/cached-tokens.md, F3).
    pub cached: Option<u32>,
    /// Which notes the user wants on the counter
    /// (`interface.show_reasoning_tokens`, `interface.show_cache_share`).
    pub notes: CounterNotes,
}

/// Which of the counter's two notes are shown (spec §11.6). Both by default:
/// the counter read that way before either could be turned off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CounterNotes {
    /// The reasoning tokens in the reply — `(reasoning 120)`.
    pub reasoning: bool,
    /// The share of the conversation read from the cache — `(cache 97%)`.
    pub cache: bool,
}

impl Default for CounterNotes {
    fn default() -> Self {
        Self {
            reasoning: true,
            cache: true,
        }
    }
}

impl CounterNotes {
    /// The notes `interface` asks for.
    pub fn of(interface: &InterfaceSettings) -> Self {
        Self {
            reasoning: interface.show_reasoning_tokens,
            cache: interface.show_cache_share,
        }
    }
}

impl TokenCounter {
    /// The forms the counter can take, widest first: the total with its notes,
    /// then the total alone — the notes are what a narrow row gives up first.
    /// Empty while there is nothing to count (no reply tokens, no conversation
    /// figure): the counter is hidden.
    pub fn forms(&self, loc: &'static Locale) -> Vec<String> {
        if self.tokens == 0 && self.context.is_none() {
            return Vec::new();
        }
        let total = (self.context.unwrap_or(0) + self.tokens).to_string();
        let approx = if self.context.is_some() && !self.context_exact {
            "~"
        } else {
            ""
        };
        let label = |reason: &str| {
            loc.tf(
                "ui.status.tokens",
                &[("approx", approx), ("total", &total), ("reason", reason)],
            )
        };
        let notes = self.notes_text(loc);
        let mut forms = vec![label(&notes)];
        if !notes.is_empty() {
            forms.push(label(""));
        }
        forms
    }

    /// The counter in full as one span, in its [`Self::style`]; `None` —
    /// nothing to count yet.
    pub fn span(&self, palette: &Palette, loc: &'static Locale) -> Option<Span<'static>> {
        let full = self.forms(loc).into_iter().next()?;
        Some(Span::styled(full, self.style(palette)))
    }

    /// Bright while the turn runs (the figure is growing), muted afterwards
    /// (the last turn's final result).
    pub fn style(&self, palette: &Palette) -> Style {
        if self.generating {
            palette.accent_style()
        } else {
            palette.muted_style()
        }
    }

    /// The notes the user kept, joined into one parenthesis — `(reasoning 120
    /// · cache 99%)` — or nothing. Reasoning tokens are included in the total
    /// and noted when there are any; the cache share is of the conversation,
    /// rounded **down** so that one missed token never reads as `100%`, and
    /// noted whenever the request reported it, `0%` included
    /// (docs/research/cached-tokens.md §4.4, F3).
    fn notes_text(&self, loc: &'static Locale) -> String {
        let mut notes = Vec::new();
        if self.notes.reasoning && self.reasoning > 0 {
            notes.push(loc.tf(
                "ui.status.tokens.reasoning",
                &[("n", &self.reasoning.to_string())],
            ));
        }
        if self.notes.cache
            && let Some(pct) = cache_share(self.cached, self.context)
        {
            notes.push(loc.tf("ui.status.tokens.cache", &[("pct", &pct.to_string())]));
        }
        if notes.is_empty() {
            String::new()
        } else {
            format!(" ({})", notes.join(" · "))
        }
    }
}

/// The cached share of `context` in whole percent, rounded down; `None` without
/// both figures or with an empty context. A count above the context (no provider
/// sends one) reads as `100`.
fn cache_share(cached: Option<u32>, context: Option<u64>) -> Option<u64> {
    let context = context.filter(|c| *c > 0)?;
    Some((u64::from(cached?) * 100 / context).min(100))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::i18n::Lang::{En, Ru};
    use crate::shared::i18n::locale;

    /// A finished turn: 8409 tokens of conversation, 5 of reply.
    fn counter(reasoning: u32, cached: Option<u32>) -> TokenCounter {
        TokenCounter {
            tokens: 5,
            context: Some(8409),
            context_exact: true,
            reasoning,
            cached,
            ..TokenCounter::default()
        }
    }

    fn full(c: &TokenCounter, lang: crate::shared::i18n::Lang) -> String {
        c.forms(locale(lang)).remove(0)
    }

    #[test]
    fn the_total_sums_the_conversation_and_the_reply() {
        let ru = locale(Ru);
        let at = |tokens, context, exact| TokenCounter {
            tokens,
            context,
            context_exact: exact,
            ..TokenCounter::default()
        };
        // Nothing to count — no form at all, and no span.
        assert!(at(0, None, false).forms(ru).is_empty());
        assert!(at(0, None, false).span(&Palette::default(), ru).is_none());
        // Reply only (conversation unknown) — no `~`.
        assert_eq!(at(42, None, false).forms(ru), ["токены: 42"]);
        // The conversation as an estimate is marked with `~`, the exact one is not.
        assert_eq!(at(42, Some(1000), false).forms(ru), ["токены: ~1042"]);
        assert_eq!(at(42, Some(1000), true).forms(ru), ["токены: 1042"]);
        // Visible right at the start: the conversation alone.
        assert_eq!(at(0, Some(1000), false).forms(ru), ["токены: ~1000"]);
    }

    /// The notes as they read (docs/research/cached-tokens.md, F3): reasoning
    /// when there is any, the cache share whenever reported, `0%` included,
    /// both in one parenthesis — and the total alone as the narrow form.
    #[test]
    fn the_notes_share_one_parenthesis() {
        // `gpt-6.1-sol` warm: 8406 of 8409 is 99.96% — rounded down, never `100%`.
        assert_eq!(
            full(&counter(0, Some(8406)), En),
            "tokens: 8414 (cache 99%)"
        );
        assert_eq!(full(&counter(0, Some(8406)), Ru), "токены: 8414 (кэш 99%)");
        // A miss is shown: it is the signal.
        assert_eq!(full(&counter(0, Some(0)), En), "tokens: 8414 (cache 0%)");
        assert_eq!(
            counter(120, Some(8192)).forms(locale(En)),
            ["tokens: 8414 (reasoning 120 · cache 97%)", "tokens: 8414"]
        );
        assert_eq!(
            full(&counter(120, Some(8192)), Ru),
            "токены: 8414 (рассужд. 120 · кэш 97%)"
        );
        // No figure for the request and no reasoning: one form, no parenthesis.
        assert_eq!(counter(0, None).forms(locale(En)), ["tokens: 8414"]);
    }

    /// Each note can be turned off on its own (spec §11.6), and the counter
    /// with both off is the bare total.
    #[test]
    fn each_note_follows_its_setting() {
        let mut c = counter(120, Some(8192));
        c.notes.reasoning = false;
        assert_eq!(full(&c, En), "tokens: 8414 (cache 97%)");
        c.notes = CounterNotes {
            reasoning: true,
            cache: false,
        };
        assert_eq!(full(&c, En), "tokens: 8414 (reasoning 120)");
        c.notes.reasoning = false;
        assert_eq!(c.forms(locale(En)), ["tokens: 8414"]);
        // The settings' defaults keep both, as the counter always read; each
        // setting is read into its own note.
        assert_eq!(
            CounterNotes::of(&InterfaceSettings::default()),
            CounterNotes::default()
        );
        let only_cache = InterfaceSettings {
            show_reasoning_tokens: false,
            ..InterfaceSettings::default()
        };
        assert_eq!(
            CounterNotes::of(&only_cache),
            CounterNotes {
                reasoning: false,
                cache: true
            }
        );
        assert!(CounterNotes::default().reasoning && CounterNotes::default().cache);
    }

    #[test]
    fn bright_while_the_turn_runs_muted_after() {
        let p = Palette::default();
        let mut c = counter(0, None);
        assert_eq!(c.span(&p, locale(Ru)).unwrap().style, p.muted_style());
        c.generating = true;
        assert_eq!(c.span(&p, locale(Ru)).unwrap().style, p.accent_style());
    }

    /// The share's arithmetic: whole percent, rounded down; nothing without
    /// both figures or against an empty conversation; a count above the
    /// conversation capped.
    #[test]
    fn the_cache_share_rounds_down_and_needs_both_figures() {
        assert_eq!(cache_share(Some(8406), Some(8409)), Some(99));
        assert_eq!(cache_share(Some(8409), Some(8409)), Some(100));
        assert_eq!(cache_share(Some(7782), Some(9825)), Some(79));
        assert_eq!(cache_share(Some(0), Some(9825)), Some(0));
        assert_eq!(cache_share(None, Some(9825)), None);
        assert_eq!(cache_share(Some(10), None), None);
        assert_eq!(cache_share(Some(10), Some(0)), None);
        assert_eq!(cache_share(Some(20), Some(10)), Some(100));
    }
}
