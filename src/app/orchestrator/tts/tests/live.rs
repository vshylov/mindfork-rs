//! Live smoke for the speech pipeline (docs/research/tts-pauses.md §5): a
//! reply shaped like the one that fell silent — headings, lists, a code block,
//! long paragraphs — spoken by a real provider through the real chunker and
//! pipeline. The sound is "played" on the clock ([`ClockQueue`]): what is
//! measured is when each chunk is ready against when the listener would need
//! it, which needs no sound card (the device itself has its own smoke,
//! `playback::tests::plays_generated_tone_live`). It takes as long as the
//! speech does — about two minutes.
//!
//! Asserted: blocks were packed, every chunk was queued, nothing was said to
//! the feed, and **no silence was the pipeline's** — a chunk fell silent only
//! when its own synthesis outlasted the sound queued when it began. The
//! deterministic claim, that the reply which fell silent now plays through, is
//! the paused-clock test over its measured timings (`pipeline.rs`).
//!
//! `MINDFORK_OPENAI_KEY=… cargo test a_structured_reply_plays_without_a_silence_live
//! -- --ignored --nocapture`; `MINDFORK_TTS_LIVE_MODEL` / `MINDFORK_TTS_LIVE_VOICE`
//! pick another model or voice (default `gpt-4o-mini-tts`, `onyx`).

use tokio::sync::mpsc::unbounded_channel;

use super::pipeline_tests::ClockQueue;
use super::*;
use crate::shared::i18n::{Lang, locale};
use crate::shared::tts::openai::OpenAiTts;

/// The reply: every short block — a heading, a list item, the code block's
/// note — is followed by a long paragraph, the order that fell silent.
const REPLY: &str = "\
Короткий ответ: да, но с оговорками. Пауза в речи появляется не из-за сети и не из-за модели, \
а из-за того, как текст режется на куски перед синтезом. Каждый абзац, заголовок и пункт списка \
раньше уходил отдельным запросом, и длинный абзац после короткого заголовка ждал свой синтез целиком.

### Что происходило

Конвейер готовил впрок только один кусок:

1. первый кусок звучит;
2. в это время синтезируется второй;
3. если второй длиннее, слушатель ждёт.

Синтез длинного абзаца занимает от трёх до двадцати семи секунд, а короткий заголовок звучит одну-две \
секунды. Разница превращается в тишину посреди ответа. Сильнее всего это заметно после списков и блоков \
кода, потому что пометка о пропущенном коде — тоже короткий блок.

```rust
fn main() {
    println!(\"this code is not read aloud\");
}
```

После блока кода обычно идёт длинное объяснение, и именно перед ним раньше возникала самая заметная \
пауза. Теперь короткие блоки склеиваются с соседями, а синтез идёт с запасом в несколько десятков секунд звука.

### Что проверить

- первый звук приходит быстро;
- между кусками нет тишины;
- остановка обрывает речь сразу.

Если всё это выполняется, исправление работает. Если пауза всё же возникла, в журнале теперь есть время \
синтеза каждого куска, длина его звука и длительность тишины, так что причину видно без догадок.

### Вывод

Паузы были свойством нарезки и конвейера, а не провайдера. Их можно убрать, не меняя голос, модель и настройки.";

#[tokio::test]
#[ignore = "requires MINDFORK_OPENAI_KEY (OpenAI speech, ~2 min of sound)"]
async fn a_structured_reply_plays_without_a_silence_live() {
    let Ok(key) = std::env::var("MINDFORK_OPENAI_KEY") else {
        eprintln!("MINDFORK_OPENAI_KEY not set — smoke skipped");
        return;
    };
    let model =
        std::env::var("MINDFORK_TTS_LIVE_MODEL").unwrap_or_else(|_| "gpt-4o-mini-tts".into());
    let voice = std::env::var("MINDFORK_TTS_LIVE_VOICE").unwrap_or_else(|_| "onyx".into());
    let engine = OpenAiTts::cloud(
        "https://api.openai.com/v1".into(),
        key,
        model.clone(),
        Some(voice.clone()),
        None,
        1.0,
    );
    let utterances = build_utterances(
        &[Message::assistant(REPLY)],
        TtsScope::Last,
        false,
        locale(Lang::Ru),
    )
    .expect("the reply has words");
    let blocks = utterances[0].1.lines().count();
    let chunks = chunk_utterances(&utterances, engine.max_input_chars());
    let requests = chunks.len();

    let (evt_tx, mut events) = unbounded_channel();
    let (done_tx, _done_rx) = unbounded_channel();
    let task = TtsTask {
        engine: Box::new(engine),
        user_engine: None,
        chunks,
        cancel: CancellationToken::new(),
        task_id: Uuid::new_v4(),
        playback: Arc::new(ClockQueue::default()),
        loc: locale(Lang::Ru),
        evt_tx,
        done_tx,
    };
    let run = tokio::time::timeout(Duration::from_secs(600), run_speech(&task))
        .await
        .expect("the run ends within ten minutes");

    eprintln!("{model}/{voice}: {blocks} blocks in {requests} requests");
    eprintln!("chunk  chars  synth_s  sound_s  ahead_s  silence_s  asked");
    for (i, c) in run.chunks.iter().enumerate() {
        eprintln!(
            "{:>5}  {:>5}  {:>7.2}  {:>7.2}  {:>7.2}  {:>9.2}  {}",
            i + 1,
            c.chars,
            c.synth.as_secs_f64(),
            c.sound.as_secs_f64(),
            c.ahead.as_secs_f64(),
            c.silence.as_secs_f64(),
            if c.hedged { "twice" } else { "once" }
        );
    }
    if let Ok(event) = events.try_recv() {
        panic!("the feed was told: {event:?}");
    }
    assert!(!run.stopped, "the run was cut short");
    assert_eq!(run.chunks.len(), requests, "every chunk was queued");
    assert!(
        requests < blocks,
        "blocks were packed: {requests} of {blocks}"
    );
    // A silence is the pipeline's only when its chunk was made in less time
    // than the sound queued when it began — asked for too late. A request that
    // outlasts all of that is the provider's, and no look-ahead hides it: one
    // took 153 s on 2026-10-24, where 3–15 s is usual (docs/research/
    // tts-pauses.md §5). It is reported, not failed — a gate that turned red on
    // the provider's bad minute would measure the provider.
    for (i, c) in run.chunks.iter().enumerate() {
        if c.silence < AUDIBLE_SILENCE {
            continue;
        }
        assert!(
            c.synth > c.ahead,
            "chunk {}: {:?} of silence, though made in {:?} with {:?} queued — asked for too late",
            i + 1,
            c.silence,
            c.synth,
            c.ahead
        );
        // And a request that outlasted both what was queued and the floor was
        // asked for a second time (docs/research/tts-pauses.md §6).
        if c.synth > c.ahead.max(HEDGE_FLOOR) + 2 * POLL_INTERVAL {
            assert!(
                c.hedged,
                "chunk {}: stalled for {:?} and asked once",
                i + 1,
                c.synth
            );
        }
        eprintln!(
            "NOTE: chunk {} took {:.1} s to make with {:.1} s queued: {:.1} s of silence, the provider's",
            i + 1,
            c.synth.as_secs_f64(),
            c.ahead.as_secs_f64(),
            c.silence.as_secs_f64()
        );
    }
    let silences = run.audible_silences().count();
    let hedges = run.chunks.iter().filter(|c| c.hedged).count();
    let sound: Duration = run.chunks.iter().map(|c| c.sound).sum();
    eprintln!(
        "GO: first sound after {:.2} s, {:.0} s of speech, {silences} silence(s), none the pipeline's; {hedges} chunk(s) asked twice",
        run.first_sound.unwrap_or_default().as_secs_f64(),
        sound.as_secs_f64()
    );
}
