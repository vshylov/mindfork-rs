//! Разбор slash-команды озвучивания в поле ввода (`/tts [N|all|stop]`). Чистая,
//! тестируемая логика по образцу [`super::rag_command`]: экран чата вызывает её при
//! отправке; распознанная команда превращается в намерение, нераспознанная строка
//! уходит обычным сообщением. См. docs/research/tts.md §7, spec §11.9.

/// Что озвучивать (снимок переписки берётся на момент команды).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TtsScope {
    /// Последнее сообщение чата (поведение по умолчанию — голое `/tts`).
    Last,
    /// Последние `n` сообщений (пользователя и модели), хронологически.
    Recent(usize),
    /// Вся переписка чата.
    All,
}

/// Распознанная команда озвучивания.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TtsCommand {
    /// Озвучить выбранный объём (прерывает текущее воспроизведение).
    Speak(TtsScope),
    /// Остановить воспроизведение (сбросить очередь).
    Stop,
    /// Приостановить воспроизведение, сохранив очередь (`/tts resume` продолжит).
    Pause,
    /// Продолжить приостановленное воспроизведение.
    Resume,
}

/// Пытается разобрать строку ввода как команду озвучивания.
///
/// - `None` — строка не является командой `/tts`: её следует отправить как обычное
///   сообщение;
/// - `Some(Ok(cmd))` — корректная команда;
/// - `Some(Err(arg))` — это `/tts`, но аргумент не распознан; `arg` — сам аргумент
///   (сообщение об ошибке формирует UI на языке интерфейса — ось B, docs/i18n-ui.md).
pub fn parse(input: &str) -> Option<Result<TtsCommand, String>> {
    let mut tokens = input.split_whitespace();
    let first = tokens.next()?;
    if !first.eq_ignore_ascii_case("/tts") {
        return None;
    }
    // Голое `/tts` — последнее сообщение (самый частый сценарий).
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
    // Число: сколько последних сообщений озвучить. Ноль бессмыслен — это ошибка,
    // а не «ничего не делать» (иначе опечатка выглядела бы как молчаливый no-op).
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
        assert_eq!(parse("обычное сообщение"), None);
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
        // Ошибка несёт сам аргумент — текст подсказки строит UI (локализованно).
        assert_eq!(parse("/tts всё"), Some(Err("всё".into())));
        assert_eq!(parse("/tts -2"), Some(Err("-2".into())));
        assert_eq!(parse("/tts 0"), Some(Err("0".into())));
        assert_eq!(parse("/tts 1.5"), Some(Err("1.5".into())));
    }

    #[test]
    fn extra_tokens_are_ignored() {
        // Лишние токены после аргумента игнорируются (как у `/rag list`).
        assert_eq!(parse("/tts all прочее"), speak(TtsScope::All));
        assert_eq!(parse("/tts 2 и ещё"), speak(TtsScope::Recent(2)));
    }
}
