//! The speech pipeline on a clock of its own (docs/research/tts-pauses.md):
//! a scripted engine whose every request takes a set time and answers a set
//! length of sound, and a queue that "plays" on tokio's paused clock — so a
//! run of minutes takes milliseconds and says exactly when the listener heard
//! nothing.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

use super::*;
use crate::shared::i18n::{Lang, locale};
use crate::shared::tts::AudioClip;

/// The rate of the scripted clips: a sample is a millisecond.
const RATE: u32 = 1_000;

/// A clip of silence `ms` long.
fn sound(ms: u64) -> AudioClip {
    AudioClip::Pcm {
        sample_rate: RATE,
        channels: 1,
        bytes: vec![0; usize::try_from(ms * 2).unwrap()],
    }
}

/// A queue that plays on the clock: each clip starts when the one before it
/// ends, or when it arrives if the queue had run dry.
#[derive(Default)]
pub(super) struct ClockQueue {
    /// Every clip's start and end, in order.
    clips: Mutex<Vec<(Instant, Instant)>>,
    stopped: std::sync::atomic::AtomicBool,
}

impl ClockQueue {
    fn end(&self) -> Option<Instant> {
        self.clips.lock().unwrap().last().map(|(_, end)| *end)
    }
}

#[async_trait::async_trait]
impl SpeechQueue for ClockQueue {
    async fn append(&self, clip: AudioClip) -> anyhow::Result<Duration> {
        let AudioClip::Pcm {
            sample_rate,
            channels,
            bytes,
        } = clip
        else {
            anyhow::bail!("the script answers raw PCM only");
        };
        let frames = bytes.len() as u64 / 2 / u64::from(channels);
        let length = Duration::from_millis(frames * 1_000 / u64::from(sample_rate));
        if length.is_zero() {
            return Ok(length);
        }
        let now = Instant::now();
        let mut clips = self.clips.lock().unwrap();
        let start = clips.last().map_or(now, |(_, end)| (*end).max(now));
        clips.push((start, start + length));
        Ok(length)
    }

    fn ahead(&self) -> Duration {
        self.end().map_or(Duration::ZERO, |end| {
            end.saturating_duration_since(Instant::now())
        })
    }

    fn is_drained(&self) -> bool {
        self.end().is_none_or(|end| end <= Instant::now())
    }

    fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.clips.lock().unwrap().clear();
    }
}

/// An engine that answers request `i` after `script[i].0` with `script[i].1`
/// of sound — or, for a request in `refused`, refuses after `script[i].0`.
/// Requests are counted in the order they are made, a second request for a
/// stalled chunk included.
struct ScriptEngine {
    script: Vec<(Duration, Duration)>,
    calls: AtomicUsize,
    refused: Vec<usize>,
}

#[async_trait::async_trait]
impl TtsEngine for ScriptEngine {
    async fn synthesize(
        &self,
        _text: &str,
        cancel: &CancellationToken,
    ) -> anyhow::Result<AudioClip> {
        let i = self.calls.fetch_add(1, Ordering::SeqCst);
        let (synth, length) = self.script[i];
        tokio::select! {
            () = cancel.cancelled() => anyhow::bail!("speech synthesis cancelled"),
            () = tokio::time::sleep(synth) => {}
        }
        if self.refused.contains(&i) {
            anyhow::bail!("the provider refused request {i}");
        }
        Ok(sound(u64::try_from(length.as_millis()).unwrap()))
    }

    fn max_input_chars(&self) -> usize {
        4096
    }
}

/// A run over `script` — `(synthesis ms, sound ms)` per request, one request
/// per chunk — into `queue`, and the feed's end of its events.
fn task(
    script: &[(u64, u64)],
    refused: &[usize],
    queue: Arc<ClockQueue>,
) -> (TtsTask<ClockQueue>, UnboundedReceiver<AppEvent>) {
    chunks_task(script.len(), script, refused, queue)
}

/// [`task`] over `chunks` chunks, the script naming every request in the
/// order made — a stalled chunk's second request too.
fn chunks_task(
    chunks: usize,
    script: &[(u64, u64)],
    refused: &[usize],
    queue: Arc<ClockQueue>,
) -> (TtsTask<ClockQueue>, UnboundedReceiver<AppEvent>) {
    let (evt_tx, evt_rx) = unbounded_channel();
    let (done_tx, _done_rx) = unbounded_channel();
    let engine = ScriptEngine {
        script: script
            .iter()
            .map(|(s, l)| (Duration::from_millis(*s), Duration::from_millis(*l)))
            .collect(),
        calls: AtomicUsize::new(0),
        refused: refused.to_vec(),
    };
    let task = TtsTask {
        engine: Box::new(engine),
        user_engine: None,
        chunks: (0..chunks)
            .map(|i| (MessageRole::Assistant, format!("chunk {i}.")))
            .collect(),
        cancel: CancellationToken::new(),
        task_id: Uuid::new_v4(),
        playback: queue,
        loc: locale(Lang::Ru),
        evt_tx,
        done_tx,
    };
    (task, evt_rx)
}

/// The run, bounded: a pipeline that never finishes fails the test instead of
/// hanging it — and on the paused clock the bound costs nothing.
async fn bounded(task: &TtsTask<ClockQueue>) -> SpeechRun {
    tokio::time::timeout(Duration::from_secs(3_600), run_speech(task))
        .await
        .expect("the run ends within an hour of the clock")
}

/// The owner's reply of 2026-10-24 as the app used to cut it — 38 chunks, one
/// per block — with each chunk's synthesis and sound **as measured** against
/// `gpt-4o-mini-tts` (docs/research/tts-pauses.md §3). Fetched one clip ahead,
/// it fell silent six times, 43.8 s in all: 21.7 s after the list's last item,
/// 11.0 s and 8.8 s after two headings.
const SESSION: [(u64, u64); 38] = [
    (4243, 25850),
    (1449, 2800),
    (1366, 3850),
    (1442, 4000),
    (1749, 8600),
    (1320, 6450),
    (2781, 4950),
    (5200, 34850),
    (9988, 25650),
    (1284, 6500),
    (2632, 600),
    (1312, 4150),
    (1155, 4000),
    (2207, 3750),
    (1172, 5200),
    (1792, 10400),
    (1188, 4250),
    (3311, 21800),
    (1772, 9750),
    (8529, 24150),
    (1151, 4200),
    (15149, 15050),
    (3609, 3450),
    (2798, 3100),
    (1216, 3450),
    (949, 2200),
    (3509, 21500),
    (8028, 15750),
    (2119, 13000),
    (1713, 8350),
    (1212, 6000),
    (1709, 7000),
    (5101, 5100),
    (26716, 23900),
    (696, 1350),
    (10068, 13300),
    (9130, 18750),
    (3215, 20350),
];

/// The defect, replayed: the same 38 chunks, the same timings, fetched by
/// seconds of sound ahead — the reply plays through without a silence, and
/// starts when the first chunk is made.
#[tokio::test(start_paused = true)]
async fn the_sessions_reply_plays_without_a_silence() {
    let queue = Arc::new(ClockQueue::default());
    let (task, _events) = task(&SESSION, &[], queue.clone());
    let run = bounded(&task).await;
    assert_eq!(run.chunks.len(), 38);
    assert!(!run.stopped);
    let silences: Vec<(usize, Duration)> = run
        .chunks
        .iter()
        .enumerate()
        .filter(|(_, c)| !c.silence.is_zero())
        .map(|(i, c)| (i, c.silence))
        .collect();
    assert!(
        silences.is_empty(),
        "silences before these chunks: {silences:?}"
    );
    assert_eq!(run.first_sound, Some(Duration::from_millis(4243)));
    // At the provider's measured speeds nothing is asked for twice: the
    // slowest chunk (26.7 s) began with more than that queued.
    assert!(
        run.chunks.iter().all(|c| !c.hedged),
        "a chunk was asked twice"
    );
    // And the queue's own record agrees: every clip starts where the one
    // before it ends.
    let clips = queue.clips.lock().unwrap();
    assert!(
        clips.windows(2).all(|w| w[1].0 == w[0].1),
        "a gap in the queue"
    );
}

/// A silence is measured, to the poll's precision, from the moment the queue
/// is found dry to the moment the next clip is in it — and counted in the
/// run's summary. The first chunk's wait is the first sound's, not a silence.
#[tokio::test(start_paused = true)]
async fn a_silence_is_measured_from_the_dry_queue_to_the_next_clip() {
    let queue = Arc::new(ClockQueue::default());
    // Two seconds of sound, then a chunk that takes nine to make — under the
    // floor, so it is not asked for twice.
    let (task, _events) = task(&[(1_000, 2_000), (9_000, 1_000)], &[], queue);
    let run = bounded(&task).await;
    assert_eq!(run.chunks[0].silence, Duration::ZERO);
    let silence = run.chunks[1].silence;
    // Dry at 3 s, the clip in at 10 s.
    let off = silence.abs_diff(Duration::from_secs(7));
    assert!(off <= POLL_INTERVAL, "{silence:?}, not ~7 s");
    assert_eq!(run.audible_silences().count(), 1);
    assert_eq!(run.first_sound, Some(Duration::from_secs(1)));
}

/// Synthesis runs ahead by seconds of sound and no further: once a chunk
/// would be made with [`LOOKAHEAD`] queued, it waits — so a stop never throws
/// away more than that and the chunk in flight.
#[tokio::test(start_paused = true)]
async fn synthesis_waits_once_the_lookahead_is_queued() {
    let queue = Arc::new(ClockQueue::default());
    // Quick to make, thirty seconds each.
    let script = [(100, 30_000); 8];
    let (task, _events) = task(&script, &[], queue);
    let run = bounded(&task).await;
    for (i, chunk) in run.chunks.iter().enumerate() {
        assert!(
            chunk.ahead < LOOKAHEAD,
            "chunk {i} made with {:?} ahead",
            chunk.ahead
        );
        assert!(chunk.silence.is_zero(), "chunk {i}: {:?}", chunk.silence);
    }
    // From the third on, each waited for the queue to come down to the bound.
    for chunk in &run.chunks[2..] {
        let below = LOOKAHEAD - chunk.ahead;
        assert!(
            below <= POLL_INTERVAL,
            "made {below:?} under the bound, not at it"
        );
    }
}

/// A run that is stopped stops synthesizing, empties the queue, and says
/// nothing to the feed — the person stopped it.
#[tokio::test(start_paused = true)]
async fn a_stopped_run_stops_the_queue_in_silence() {
    let queue = Arc::new(ClockQueue::default());
    let (task, mut events) = task(
        &[(1_000, 5_000), (60_000, 5_000), (1_000, 5_000)],
        &[],
        queue.clone(),
    );
    let cancel = task.cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(3)).await;
        cancel.cancel();
    });
    let run = bounded(&task).await;
    assert!(run.stopped);
    assert_eq!(run.chunks.len(), 1, "only the first chunk was queued");
    assert!(
        queue.stopped.load(Ordering::SeqCst),
        "the queue was stopped"
    );
    assert!(events.try_recv().is_err(), "nothing said to the feed");
}

/// A refused synthesis ends the run with the provider's words in the feed,
/// after what was queued has played.
#[tokio::test(start_paused = true)]
async fn a_refused_chunk_ends_the_run_and_says_why() {
    let queue = Arc::new(ClockQueue::default());
    let (task, mut events) = task(
        &[(1_000, 5_000), (1_000, 5_000), (1_000, 5_000)],
        &[1],
        queue.clone(),
    );
    let run = bounded(&task).await;
    assert!(run.stopped);
    assert_eq!(run.chunks.len(), 1);
    match events.try_recv() {
        Ok(AppEvent::Error(msg)) => assert!(msg.contains("the provider refused"), "{msg}"),
        other => panic!("expected the refusal in the feed, got {other:?}"),
    }
    assert!(
        !queue.stopped.load(Ordering::SeqCst),
        "what was queued was played, not cut"
    );
}

/// A log writer the test reads back.
#[derive(Clone, Default)]
struct LogBuf(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogBuf {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The log answers what the owner had to reproduce by hand: every chunk's
/// synthesis, sound and the silence before it, and the run's summary.
#[tokio::test(start_paused = true)]
async fn the_log_tells_each_chunk_and_the_runs_silences() {
    let log = LogBuf::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    // tracing decides once, for every thread, whether a line is wanted; with a
    // single scoped subscriber it asks only the thread that reaches the line
    // first — a second one kept alive makes it ask every thread's
    // (docs/lessons.md §2).
    let _second = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
    // The run is spawned on this test's thread (a current-thread runtime).
    let _guard = tracing::subscriber::set_default(subscriber);

    // Two runs, one after the other: one with a silence, one with a stall.
    let runs = [
        (2, vec![(1_000, 2_000), (9_000, 1_000)]),
        (2, vec![(1_000, 2_000), (120_000, 5_000), (3_000, 4_000)]),
    ];
    for (chunks, script) in runs {
        let queue = Arc::new(ClockQueue::default());
        let (mut task, _events) = chunks_task(chunks, &script, &[], queue);
        let (done_tx, mut done_rx) = unbounded_channel();
        task.done_tx = done_tx;
        let id = task.task_id;
        spawn_tts(task);
        let done = tokio::time::timeout(Duration::from_secs(3_600), done_rx.recv())
            .await
            .expect("the run ends");
        assert_eq!(done, Some(id));
    }

    let out = String::from_utf8_lossy(&log.0.lock().unwrap()).into_owned();
    let line = |what: &str, n: usize| -> String {
        out.lines()
            .filter(|l| l.contains(what))
            .nth(n)
            .unwrap_or_else(|| panic!("no line {n} of {what:?}: {out}"))
            .to_string()
    };
    let first = line("speech: a chunk is queued", 0);
    for field in [
        "chunk=1",
        "of=2",
        "synth_ms=1000",
        "sound_ms=2000",
        "silence_ms=0",
        "hedged=false",
    ] {
        assert!(first.contains(field), "{field} missing: {first}");
    }
    let second = line("speech: a chunk is queued", 1);
    for field in [
        "chunk=2",
        "synth_ms=9000",
        "sound_ms=1000",
        "ahead_ms=2000",
        "hedged=false",
    ] {
        assert!(second.contains(field), "{field} missing: {second}");
    }
    let ended = line("speech: the run ended", 0);
    for field in [
        "chunks=2",
        "sound_s=3",
        "first_sound_ms=1000",
        "silences=1",
        "hedges=0",
        "stopped=false",
    ] {
        assert!(ended.contains(field), "{field} missing: {ended}");
    }
    // The stalled run: the second request is told when it goes, on the
    // chunk's line, and in the summary — a bill's extra request can be found.
    let stalled = line("speech: a chunk's synthesis stalled; asking again", 0);
    assert!(stalled.contains("waited_ms=10000"), "{stalled}");
    let answered = line("speech: a chunk asked twice is answered", 0);
    for field in ["by=\"the second request\"", "ok=true", "waited_ms=13000"] {
        assert!(answered.contains(field), "{field} missing: {answered}");
    }
    let hedged = line("speech: a chunk is queued", 3);
    assert!(hedged.contains("hedged=true"), "{hedged}");
    let ended_stalled = line("speech: the run ended", 1);
    assert!(ended_stalled.contains("hedges=1"), "{ended_stalled}");
    // The silence itself: dry at 3 s, the clip in at 10 s, to the poll.
    let silence: u64 = ended
        .split("silence_ms=")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("no silence_ms: {ended}"));
    assert!((6_920..=7_080).contains(&silence), "{silence} ms: {ended}");
}

/// A request that outlasts the sound queued when it began — a provider stall,
/// measured once at 153 s — is asked a second time, and the first answer is
/// kept: the 118 s of silence it would have made become 11.
#[tokio::test(start_paused = true)]
async fn a_stalled_chunk_is_asked_again_and_the_first_answer_kept() {
    let queue = Arc::new(ClockQueue::default());
    // Chunk 1 begins at 1 s with 2 s queued; its request stalls for 120 s.
    // Asked again at the floor (10 s), the second request answers in 3.
    let script = [(1_000, 2_000), (120_000, 5_000), (3_000, 4_000)];
    let (task, mut events) = chunks_task(2, &script, &[], queue);
    let run = bounded(&task).await;
    assert!(!run.stopped);
    assert!(!run.chunks[0].hedged);
    let stalled = run.chunks[1];
    assert!(stalled.hedged, "the stalled chunk was asked again");
    // The second request's answer, not the stalled one's.
    assert_eq!(stalled.sound, Duration::from_secs(4));
    // Dry at 3 s; asked again at 11 s (to the poll), answered at 14 s.
    let off = stalled.silence.abs_diff(Duration::from_secs(11));
    assert!(off <= 2 * POLL_INTERVAL, "{:?}, not ~11 s", stalled.silence);
    assert!(
        stalled.synth < Duration::from_secs(14),
        "{:?}",
        stalled.synth
    );
    assert!(events.try_recv().is_err(), "nothing said to the feed");
}

/// The rule is the user's: a request is asked again only once it outlasts
/// what was queued when it began. A slow chunk that the queue still covers is
/// not.
#[tokio::test(start_paused = true)]
async fn a_slow_chunk_the_queue_covers_is_not_asked_twice() {
    let queue = Arc::new(ClockQueue::default());
    // Chunk 1 begins with 30 s queued and takes 25 s: slow, and covered.
    let (task, _events) = task(&[(1_000, 30_000), (25_000, 10_000)], &[], queue);
    let run = bounded(&task).await;
    assert_eq!(run.chunks.len(), 2);
    assert!(run.chunks.iter().all(|c| !c.hedged), "{:?}", run.chunks);
    assert!(run.chunks[1].silence.is_zero());
}

/// The floor: with nothing queued — the run's first chunk — a request is not
/// asked again before 10 s; past them, it is.
#[tokio::test(start_paused = true)]
async fn the_first_chunk_waits_for_the_floor_before_asking_again() {
    let queue = Arc::new(ClockQueue::default());
    let (task, _events) = chunks_task(1, &[(9_000, 2_000)], &[], queue);
    let run = bounded(&task).await;
    assert!(!run.chunks[0].hedged, "9 s is under the floor");

    let queue = Arc::new(ClockQueue::default());
    let (task, _events) = chunks_task(1, &[(30_000, 2_000), (2_000, 3_000)], &[], queue);
    let run = bounded(&task).await;
    assert!(run.chunks[0].hedged, "30 s is past it");
    assert_eq!(
        run.chunks[0].sound,
        Duration::from_secs(3),
        "the second answered first"
    );
    assert_eq!(run.first_sound, Some(Duration::from_secs(12)));
}

/// The first answer wins whichever request gives it: a stalled request that
/// answers before the second is kept.
#[tokio::test(start_paused = true)]
async fn the_original_request_wins_if_it_answers_first() {
    let queue = Arc::new(ClockQueue::default());
    // Asked again at 10 s; the original answers at 12 s, the second would at 40.
    let (task, _events) = chunks_task(1, &[(12_000, 2_000), (30_000, 9_000)], &[], queue);
    let run = bounded(&task).await;
    assert!(run.chunks[0].hedged);
    assert_eq!(
        run.chunks[0].sound,
        Duration::from_secs(2),
        "the original's answer"
    );
    assert_eq!(run.first_sound, Some(Duration::from_secs(12)));
}

/// A refusal while the other request runs waits for the other; only both
/// refused end the run — with the later refusal's words.
#[tokio::test(start_paused = true)]
async fn one_refusal_waits_for_the_other_and_two_end_the_run() {
    // The second request is refused at once; the stalled first answers at 50 s.
    let queue = Arc::new(ClockQueue::default());
    let (task, mut events) = chunks_task(1, &[(50_000, 2_000), (100, 0)], &[1], queue);
    let run = bounded(&task).await;
    assert!(!run.stopped, "the other request answered");
    assert_eq!(run.chunks[0].sound, Duration::from_secs(2));
    assert!(events.try_recv().is_err(), "nothing said to the feed");

    // Both refused: the first at 30 s, the second at 40 s (asked at 10).
    let queue = Arc::new(ClockQueue::default());
    let (task, mut events) = chunks_task(1, &[(30_000, 2_000), (30_000, 2_000)], &[0, 1], queue);
    let run = bounded(&task).await;
    assert!(run.stopped);
    assert!(run.chunks.is_empty());
    match events.try_recv() {
        Ok(AppEvent::Error(msg)) => assert!(msg.contains("refused request 1"), "{msg}"),
        other => panic!("expected the later refusal in the feed, got {other:?}"),
    }
}

/// A stop ends both requests, in silence.
#[tokio::test(start_paused = true)]
async fn a_stop_ends_both_requests() {
    let queue = Arc::new(ClockQueue::default());
    let (task, mut events) = chunks_task(1, &[(60_000, 2_000), (60_000, 2_000)], &[], queue);
    let cancel = task.cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(15)).await;
        cancel.cancel();
    });
    let started = Instant::now();
    let run = bounded(&task).await;
    assert!(run.stopped);
    assert!(run.chunks.is_empty());
    assert!(
        started.elapsed() < Duration::from_secs(16),
        "{:?}",
        started.elapsed()
    );
    assert!(events.try_recv().is_err(), "nothing said to the feed");
}

/// Through the real client, against a server that never answers the first
/// request: the second is sent at `hedge_at`, its answer kept, and the first
/// request's connection **closed** — so the provider stops making what nobody
/// will hear. Real sockets, so a real clock, bounded everywhere.
#[tokio::test]
async fn a_stalled_request_is_dropped_on_the_wire_when_the_second_answers() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use crate::shared::http_stub::whole_request;
    use crate::shared::tts::openai::OpenAiTts;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let bound = Duration::from_secs(10);
    let server = tokio::spawn(async move {
        // The first request: read, never answered.
        let (mut stalled, _) = tokio::time::timeout(bound, listener.accept())
            .await
            .expect("the first request")
            .unwrap();
        let first = whole_request(&mut stalled).await;
        // The second: answered with a second of raw PCM.
        let (mut answered, _) = tokio::time::timeout(bound, listener.accept())
            .await
            .expect("the second request")
            .unwrap();
        let second = whole_request(&mut answered).await;
        let pcm = vec![0u8; 48_000];
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: audio/pcm\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            pcm.len()
        );
        answered.write_all(head.as_bytes()).await.unwrap();
        answered.write_all(&pcm).await.unwrap();
        answered.flush().await.unwrap();
        // The client must hang up on the first: a read sees the end.
        let mut buf = [0u8; 64];
        let hung_up = tokio::time::timeout(bound, stalled.read(&mut buf)).await;
        (first, second, hung_up.map(|r| r.map_err(|e| e.kind())))
    });

    let engine = OpenAiTts::cloud(url, "test-key".into(), "tts-model".into(), None, None, 1.0);
    let cancel = CancellationToken::new();
    let mut ticks = 0;
    let (result, hedged) = tokio::time::timeout(
        bound,
        synthesize_hedged(
            &engine,
            "Проверка.",
            &cancel,
            Duration::from_millis(300),
            || {
                ticks += 1;
            },
        ),
    )
    .await
    .expect("an answer within the bound");
    assert!(hedged, "the second request was sent");
    assert!(ticks > 0, "the caller's tick ran while it waited");
    match result.expect("the second request's answer") {
        AudioClip::Pcm { bytes, .. } => assert_eq!(bytes.len(), 48_000),
        other => panic!("raw PCM expected, got {other:?}"),
    }
    let (first, second, hung_up) = server.await.unwrap();
    assert_eq!(
        crate::shared::http_stub::body_of(&first),
        crate::shared::http_stub::body_of(&second),
        "the same chunk asked twice"
    );
    assert_eq!(
        hung_up,
        Ok(Ok(0)),
        "the stalled request's connection was closed"
    );
}
