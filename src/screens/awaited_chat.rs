//! The chat a screen asked for and is staying on screen to wait for. See spec §11.2.
//!
//! Opening a chat is a round trip: the screen says which one, the orchestrator
//! (the owner of `Chat`) answers with `ChatActivated`, and only then does the
//! chat screen hold it. The UI loop picks that answer up on its **next** pass,
//! a tick later — so a screen that gave way to the chat at once showed the
//! *previous* conversation for that tick, a whole frame of the wrong chat. The
//! screen that asked therefore stays up until the answer arrives, and the
//! transition is one frame: this screen → the chat that was asked for.
//!
//! It lives on the screen rather than beside `active` in the loop because it
//! has to die with the screen: closing the list by `Esc` is also the end of
//! waiting, with nothing to remember to clear.

use uuid::Uuid;

/// What a screen is waiting for, if anything.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum AwaitedChat {
    /// Nothing was asked for — an activation only updates the screen.
    #[default]
    Nothing,
    /// An existing chat: only **its own** activation is the awaited one. An
    /// unrelated activation arriving meanwhile (the active chat deleted, a
    /// background landing) must not take the screen down.
    Chat(Uuid),
    /// A chat the orchestrator is about to create (`Ctrl+N`, a clone): its id
    /// is not known here, so the next activation is taken to be it.
    Created,
}

impl AwaitedChat {
    /// Whether the activation of `id` is the one being waited for. Consumed when
    /// it is: a later re-activation of the same chat must not close a screen
    /// the user has since opened again.
    pub fn take_if_arrived(&mut self, id: Uuid) -> bool {
        let arrived = match *self {
            Self::Nothing => false,
            Self::Chat(awaited) => awaited == id,
            Self::Created => true,
        };
        if arrived {
            *self = Self::Nothing;
        }
        arrived
    }

    /// Stops waiting: the request was refused, or the user has moved on (any
    /// further key on the screen). The activation, if it still comes, then
    /// finds a screen that is simply open.
    pub fn clear(&mut self) {
        *self = Self::Nothing;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_existing_chat_is_awaited_by_id_and_taken_once() {
        let (asked, other) = (Uuid::new_v4(), Uuid::new_v4());
        let mut awaited = AwaitedChat::Chat(asked);
        assert!(
            !awaited.take_if_arrived(other),
            "somebody else's activation"
        );
        assert_eq!(awaited, AwaitedChat::Chat(asked), "still waiting");
        assert!(awaited.take_if_arrived(asked));
        assert!(!awaited.take_if_arrived(asked), "consumed");
    }

    #[test]
    fn a_created_chat_is_whatever_activates_next() {
        let mut awaited = AwaitedChat::Created;
        assert!(awaited.take_if_arrived(Uuid::new_v4()));
        assert!(!awaited.take_if_arrived(Uuid::new_v4()), "consumed");
    }

    #[test]
    fn nothing_awaited_and_a_cleared_wait_take_nothing() {
        let id = Uuid::new_v4();
        assert!(!AwaitedChat::default().take_if_arrived(id));
        let mut awaited = AwaitedChat::Chat(id);
        awaited.clear();
        assert!(!awaited.take_if_arrived(id));
    }
}
