//! Where a conversation's feed stands once it is activated
//! (docs/research/feed-landing.md §3.1, spec §11.3).
//!
//! Every opening and every rebuild of the feed is one activation
//! ([`AppEvent::ChatActivated`](crate::app::events::AppEvent::ChatActivated)),
//! and each route into it knows where the person should be looking: the end
//! of a conversation they picked from the list, the start of one a reference
//! named, the message whose variant they switched, the row they left when
//! they followed a reference out. This is that knowledge as a value — plain
//! data the orchestrator passes through and the chat screen applies; defined
//! here, below both, and re-exported by `app::events` (docs/architecture.md
//! §2, docs/research/fsd-layer-gate.md).

use uuid::Uuid;

/// A row of a conversation's feed that can be found again after the feed is
/// rebuilt (research §3.4).
///
/// The screen reads it from the frame last drawn and gives it back with a
/// later activation. It is **applied once**: a row inside a block is only
/// right at the width it was read at, so the feed clamps it to the block and
/// from there ties the view to the message, as it does after a jump
/// (docs/history/chat-search-stage2.md, fork S6). Session state — never
/// written to disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeedPlace {
    /// The first message of the bubble holding the row.
    pub message: Uuid,
    /// The row inside that bubble's block, counted from its first.
    pub row: usize,
}

/// Where the feed stands once a chat is activated (research §3.2).
///
/// A landing the feed cannot honour — a message it does not show (a `Tool`
/// or `System` one, one that left the line, one from another chat) — is
/// [`Self::Tail`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum FeedLanding {
    /// The end of the conversation, following it: an ordinary opening, and a
    /// rebuild after the person cut the line's tail (`Ctrl+E`, `Ctrl+R`,
    /// `/fork`).
    #[default]
    Tail,
    /// The first row: a followed `chat://` reference, a transcript that is
    /// not running, a swapped greeting.
    Top,
    /// On a message, its rail marked, the query's matches highlighted inside
    /// it — a search hit (docs/history/chat-search-stage2.md §3, S3(b)). An
    /// empty query highlights nothing.
    ///
    /// One variant rather than a target and an optional query because the
    /// halves are one thought: a query with no message has nothing to mean.
    Jump { message: Uuid, query: String },
    /// On a message whose content has just changed under the person — a
    /// variant switch. Not moved when the message's first row is on the
    /// screen, so the mark that was clicked stays under the pointer; put on
    /// the view's first row otherwise (research §3.3).
    Reveal { message: Uuid },
    /// Where the person was: the way back from a followed reference
    /// (research §3.5).
    Place(FeedPlace),
    /// Nothing of the person's is touched: the open chat got rows by itself —
    /// a background run's result, a follow-up, rows from the messenger, a
    /// reply that chose silence — and the feed is rebuilt to show them
    /// (research §3.6). A feed that follows its tail still does; one that does
    /// not keeps the rows it shows; the in-feed search, the selection, a
    /// jump's mark, the open lists and the input box stay as they are. For a
    /// chat that is not the open one there is nothing to hold, and it is
    /// [`Self::Tail`].
    Hold,
}
