//! Parses the speech slash-command in the input box (`/tts [N|all|stop]`). Pure,
//! testable logic modeled on [`super::rag_command`]: the chat screen calls it on
//! send; a recognized command turns into an intent, an unrecognized string
//! goes out as a regular message. See docs/research/tts.md §7, spec §11.9.

/// What to speak (a conversation snapshot is taken at command time).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TtsScope {
    /// The chat's last message (the default behavior — a bare `/tts`).
    Last,
    /// The last `n` messages (user and model), chronologically.
    Recent(usize),
    /// The whole chat conversation.
    All,
}

/// A recognized speech command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TtsCommand {
    /// Speak the selected scope (interrupts the current playback).
    Speak(TtsScope),
    /// Stop playback (clear the queue).
    Stop,
    /// Pause playback, keeping the queue (`/tts resume` will continue it).
    Pause,
    /// Resume paused playback.
    Resume,
}

/// Tries to parse an input string as a speech command.
///
/// - `None` — the string isn't a `/tts` command: it should be sent as a regular
///   message;
/// - `Some(Ok(cmd))` — a correct command;
/// - `Some(Err(arg))` — this is `/tts`, but the argument isn't recognized; `arg` — the argument
///   itself (the UI builds the error message in the interface language — axis B, docs/i18n-ui.md).
pub fn parse(input: &str) -> Option<Result<TtsCommand, String>> {
    let mut tokens = input.split_whitespace();
    let first = tokens.next()?;
    if !first.eq_ignore_ascii_case("/tts") {
        return None;
    }
    // A bare `/tts` — the last message (the most common scenario).
    let Some(arg) = tokens.next() else {
        return Some(Ok(TtsCommand::Speak(TtsScope::Last)));
    };
    if arg.eq_ignore_ascii_case("all") {
        return Some(Ok(TtsCommand::Speak(TtsScope::All)));
    }
    if arg.eq_ignore_ascii_case("stop") {
        return Some(Ok(TtsCommand::Stop));
    }
    if arg.eq_ignore_ascii_case("pause") {
        return Some(Ok(TtsCommand::Pause));
    }
    if arg.eq_ignore_ascii_case("resume") {
        return Some(Ok(TtsCommand::Resume));
    }
    // A number: how many recent messages to speak. Zero is meaningless — it's an error,
    // not "do nothing" (otherwise a typo would look like a silent no-op).
    match arg.parse::<usize>() {
        Ok(n) if n > 0 => Some(Ok(TtsCommand::Speak(TtsScope::Recent(n)))),
        _ => Some(Err(arg.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn speak(scope: TtsScope) -> Option<Result<TtsCommand, String>> {
        Some(Ok(TtsCommand::Speak(scope)))
    }

    #[test]
    fn non_tts_input_is_none() {
        assert_eq!(parse("regular message"), None);
        assert_eq!(parse("/rag list"), None);
        assert_eq!(parse("/ttsx"), None);
        assert_eq!(parse(""), None);
    }

    #[test]
    fn bare_command_speaks_last_message() {
        assert_eq!(parse("/tts"), speak(TtsScope::Last));
        assert_eq!(parse("   /tts   "), speak(TtsScope::Last));
    }

    #[test]
    fn parses_count_all_and_stop() {
        assert_eq!(parse("/tts 3"), speak(TtsScope::Recent(3)));
        assert_eq!(parse("/tts 1"), speak(TtsScope::Recent(1)));
        assert_eq!(parse("/tts all"), speak(TtsScope::All));
        assert_eq!(parse("/tts stop"), Some(Ok(TtsCommand::Stop)));
        assert_eq!(parse("/tts pause"), Some(Ok(TtsCommand::Pause)));
        assert_eq!(parse("/tts resume"), Some(Ok(TtsCommand::Resume)));
    }

    #[test]
    fn command_and_args_are_case_insensitive() {
        assert_eq!(parse("/TTS ALL"), speak(TtsScope::All));
        assert_eq!(parse("/Tts Stop"), Some(Ok(TtsCommand::Stop)));
        assert_eq!(parse("/Tts Pause"), Some(Ok(TtsCommand::Pause)));
        assert_eq!(parse("/TTS RESUME"), Some(Ok(TtsCommand::Resume)));
    }

    #[test]
    fn unknown_argument_reports_itself() {
        // The error carries the argument itself — the UI builds the hint text (localized).
        assert_eq!(parse("/tts всё"), Some(Err("всё".into())));
        assert_eq!(parse("/tts -2"), Some(Err("-2".into())));
        assert_eq!(parse("/tts 0"), Some(Err("0".into())));
        assert_eq!(parse("/tts 1.5"), Some(Err("1.5".into())));
    }

    #[test]
    fn extra_tokens_are_ignored() {
        // Extra tokens after the argument are ignored (as with `/rag list`).
        assert_eq!(parse("/tts all прочее"), speak(TtsScope::All));
        assert_eq!(parse("/tts 2 и ещё"), speak(TtsScope::Recent(2)));
    }
}
