//! Playing synthesized speech via `rodio` **inside the application process**.
//!
//! Why in-process rather than a sidecar player: an instantly cancellable
//! source queue is needed, and Windows has no suitable built-in CLI player
//! (see docs/research/tts.md §6). `Player::append` plays sources
//! sequentially — this gives us, for free, a pipeline of "synthesizing chunk
//! N+1 while N plays", with silence until the next chunk once the queue drains.
//!
//! Two TUI pitfalls, both accounted for here:
//! 1. `log_on_drop(false)` — otherwise rodio prints to stderr **over the TUI**;
//! 2. on Linux libasound itself is noisy on stderr while enumerating devices
//!    (cpal#384) — so the device is opened **lazily**, only on the first
//!    `/tts` command (not at app startup), and an open failure is an `Err`,
//!    not a panic (headless/CI/a machine with no sound card → graceful
//!    degradation, like `UnavailableEmbedder`, ADR 0002).
//!
//! The queue knows **how much sound it still holds** ([`Playback::ahead`]):
//! the pipeline synthesizes ahead by seconds of audio, not by clips, since a
//! two-second heading and a thirty-second paragraph are one clip each
//! (docs/research/tts-pauses.md F1, F9).

use std::collections::VecDeque;
use std::io::Cursor;
use std::num::NonZero;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use anyhow::{Context, Result};
use rodio::{DeviceSinkBuilder, MixerDeviceSink, Player, Source};

use super::AudioClip;

/// What the speech pipeline plays into: the sound card's queue
/// ([`Playback`]), or a test's clock.
#[async_trait::async_trait]
pub trait SpeechQueue: Send + Sync + 'static {
    /// Puts a clip at the end of the queue; its length. An empty clip queues
    /// nothing and is zero long.
    async fn append(&self, clip: AudioClip) -> Result<Duration>;
    /// The sound queued and not yet played — the clip that is playing counted
    /// from where it is. It does not shrink while playback is paused.
    fn ahead(&self) -> Duration;
    /// Whether everything queued has played.
    fn is_drained(&self) -> bool;
    /// Stops at once and drops the queue.
    fn stop(&self);
}

/// An open audio device with a playback queue.
///
/// The handle and the player are `Send + Sync` (cpal 0.17), so they live
/// right inside the speech background tokio task — no dedicated audio thread
/// is needed. On drop the device closes, the queue vanishes along with the task.
pub struct Playback {
    /// Keeps the device alive: dropping it stops the sound. `None` only in a
    /// test, whose player is pulled by hand.
    _sink: Option<MixerDeviceSink>,
    player: Player,
    /// The length of every clip appended, oldest first. The player counts
    /// what it still holds (`len`), so the last that many are the clips still
    /// queued; the older ones are dropped as they are found played.
    lengths: Mutex<VecDeque<Duration>>,
}

/// A clip as the player takes it: interleaved samples at a rate.
struct Decoded {
    channels: NonZero<u16>,
    rate: NonZero<u32>,
    samples: Vec<f32>,
}

impl Decoded {
    /// A clip's sound, or `None` for a clip with nothing in it. A container
    /// is decoded **whole** here — the only way to know its length, since a
    /// streamed MP3's header may count none of its frames, or half
    /// ([`decoder`]); a length of zero would let the pipeline synthesize
    /// everything ahead.
    fn from_clip(clip: AudioClip) -> Result<Option<Self>> {
        let decoded = match clip {
            AudioClip::Pcm {
                sample_rate,
                channels,
                bytes,
            } => {
                // Raw PCM bypasses the decoder: s16le → f32 and straight into the buffer.
                let samples = pcm_s16le_to_f32(&bytes);
                if samples.is_empty() {
                    return Ok(None);
                }
                Decoded {
                    channels: NonZero::new(channels).context("zero channel count")?,
                    rate: NonZero::new(sample_rate).context("zero sample rate")?,
                    samples,
                }
            }
            AudioClip::Encoded(bytes) => {
                if bytes.is_empty() {
                    return Ok(None);
                }
                let source = decoder(bytes)?;
                Decoded {
                    channels: source.channels(),
                    rate: source.sample_rate(),
                    samples: source.collect(),
                }
            }
        };
        Ok((!decoded.samples.is_empty()).then_some(decoded))
    }

    /// How long the clip plays.
    fn length(&self) -> Duration {
        let frames = self.samples.len() as f64 / f64::from(self.channels.get());
        Duration::from_secs_f64(frames / f64::from(self.rate.get()))
    }
}

impl Playback {
    /// Opens the default device and sets up the queue.
    ///
    /// Returns `Err` (rather than panicking) when there's no audio: a
    /// headless environment, CI, a missing/busy device. The caller shows this as a note.
    pub fn open() -> Result<Self> {
        let mut sink =
            DeviceSinkBuilder::open_default_sink().context("failed to open the audio device")?;
        // Otherwise rodio writes to stderr on drop, and the TUI occupies it.
        sink.log_on_drop(false);
        let player = Player::connect_new(sink.mixer());
        Ok(Self {
            _sink: Some(sink),
            player,
            lengths: Mutex::new(VecDeque::new()),
        })
    }

    /// A queue with no device behind it, and the end its sound comes out of —
    /// a test plays it by pulling samples.
    #[cfg(test)]
    fn detached() -> (Self, rodio::queue::SourcesQueueOutput) {
        let (player, output) = Player::new();
        let playback = Self {
            _sink: None,
            player,
            lengths: Mutex::new(VecDeque::new()),
        };
        (playback, output)
    }

    /// Puts a clip at the end of the queue, decoding it on the caller's
    /// thread; its length. It starts playing once the previous ones finish.
    /// The pipeline goes through [`SpeechQueue::append`], which decodes off
    /// the runtime's threads; this is the tests' door.
    #[cfg(test)]
    pub fn enqueue(&self, clip: AudioClip) -> Result<Duration> {
        Ok(self.append_decoded(Decoded::from_clip(clip)?))
    }

    fn append_decoded(&self, decoded: Option<Decoded>) -> Duration {
        let Some(decoded) = decoded else {
            return Duration::ZERO;
        };
        let length = decoded.length();
        let mut lengths = self.lengths.lock().unwrap_or_else(PoisonError::into_inner);
        self.player.append(rodio::buffer::SamplesBuffer::new(
            decoded.channels,
            decoded.rate,
            decoded.samples,
        ));
        lengths.push_back(length);
        length
    }

    /// The sound still queued: the clips the player holds, less how far into
    /// the first of them it is. Right after a clip is appended to an empty
    /// queue the player may still report the position it reached in the clip
    /// before (it updates every 5 ms), which reads as less sound than there is
    /// — the safe side: one chunk synthesized early.
    pub fn ahead(&self) -> Duration {
        let mut lengths = self.lengths.lock().unwrap_or_else(PoisonError::into_inner);
        let held = self.player.len();
        while lengths.len() > held {
            lengths.pop_front();
        }
        let queued: Duration = lengths.iter().sum();
        queued.saturating_sub(self.player.get_pos())
    }

    /// Whether everything has played (the queue is empty).
    pub fn is_drained(&self) -> bool {
        self.player.empty()
    }

    /// Immediately stops playback and clears the queue.
    pub fn stop(&self) {
        self.player.clear();
    }

    /// Pauses playback, **keeping the queue** (unlike
    /// [`stop`](Self::stop)). The sound card stops consuming samples;
    /// synthesizing ahead holds itself back (the queue doesn't drain), and
    /// the background task doesn't finish (`is_drained` stays `false`). Idempotent.
    pub fn pause(&self) {
        self.player.pause();
    }

    /// Resumes playback after [`pause`](Self::pause). Idempotent (resuming
    /// an already-playing player is a no-op).
    pub fn resume(&self) {
        self.player.play();
    }
}

#[async_trait::async_trait]
impl SpeechQueue for Playback {
    /// A clip of minutes is milliseconds to seconds of decoding (more in a
    /// debug build), so it is decoded on the blocking pool and only appended
    /// here.
    async fn append(&self, clip: AudioClip) -> Result<Duration> {
        let decoded = tokio::task::spawn_blocking(move || Decoded::from_clip(clip))
            .await
            .context("decoding a speech clip")??;
        Ok(self.append_decoded(decoded))
    }

    fn ahead(&self) -> Duration {
        Playback::ahead(self)
    }

    fn is_drained(&self) -> bool {
        Playback::is_drained(self)
    }

    fn stop(&self) {
        Playback::stop(self);
    }
}

/// The decoder of a container a speech server answered with.
///
/// **Not gapless.** Gapless playback trims what the first frame's `Info` tag
/// says the encoder added, and counts the frames by the same tag — and a speech
/// server writes that tag before it knows how long the stream will be. Measured
/// on the MP3 of the OpenRouter gateway's twelve models that answer one
/// (2026-09-29, docs/research/openrouter-mode.md §13): MiniMax's tag counts no
/// frames at all, and the demuxer subtracts the encoder's delay from that — an
/// overflow, which is a panic in a build that checks for one; Kokoro's counts
/// half of them, and its sentence stopped at 2.35 s of 4.78. Untrimmed, a clip
/// begins with the encoder's few dozen milliseconds of silence, which a
/// sentence read aloud does not miss.
pub(super) fn decoder(bytes: Vec<u8>) -> Result<rodio::Decoder<Cursor<Vec<u8>>>> {
    rodio::Decoder::builder()
        .with_data(Cursor::new(bytes))
        .with_gapless(false)
        .build()
        .context("failed to decode audio from the TTS server")
}

/// Signed 16-bit little-endian PCM → `f32` samples in the range −1.0…1.0
/// (rodio's internal representation). An odd trailing byte is dropped.
fn pcm_s16le_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm_conversion_scales_and_drops_odd_tail() {
        // 0 → 0.0; i16::MIN → −1.0; 16384 → 0.5. The odd tail is dropped.
        let bytes = [0x00, 0x00, 0x00, 0x80, 0x00, 0x40, 0x7f];
        let samples = pcm_s16le_to_f32(&bytes);
        assert_eq!(samples, vec![0.0, -1.0, 0.5]);
        assert!(pcm_s16le_to_f32(&[]).is_empty());
        assert!(pcm_s16le_to_f32(&[0x01]).is_empty());
    }

    /// Samples in a frame of MPEG-1 Layer III.
    const PER_FRAME: usize = 1152;

    /// An MP3 of silence the way a streaming encoder writes one: a first frame
    /// that holds no audio but an `Info` tag — which counts `claimed` frames
    /// and names the encoder's delay — and `frames` frames after it. Mono,
    /// 44.1 kHz, 128 kbit/s.
    fn streamed_mp3(frames: usize, claimed: u32) -> Vec<u8> {
        const HEADER: [u8; 4] = [0xFF, 0xFB, 0x90, 0xC4];
        // 144 × 128000 / 44100, no padding.
        const FRAME: usize = 417;
        // A mono frame's side information, which the tag stands after.
        const SIDE_INFO: usize = 17;
        let mut tag = Vec::new();
        tag.extend_from_slice(b"Info");
        // Flags: the frame count follows, and nothing else.
        tag.extend_from_slice(&1u32.to_be_bytes());
        tag.extend_from_slice(&claimed.to_be_bytes());
        tag.extend_from_slice(b"Lavc58.13");
        // Revision, lowpass, peak, two replay gains, flags, bitrate.
        tag.extend_from_slice(&[0; 12]);
        // The encoder's delay, 576 samples, in the upper twelve bits of three
        // bytes; no padding in the lower twelve.
        tag.extend_from_slice(&[0x24, 0x00, 0x00]);

        let mut out = Vec::with_capacity(FRAME * (frames + 1));
        let mut first = vec![0u8; FRAME];
        first[..4].copy_from_slice(&HEADER);
        first[4 + SIDE_INFO..4 + SIDE_INFO + tag.len()].copy_from_slice(&tag);
        out.extend_from_slice(&first);
        for _ in 0..frames {
            let mut frame = vec![0u8; FRAME];
            frame[..4].copy_from_slice(&HEADER);
            out.extend_from_slice(&frame);
        }
        out
    }

    /// A streamed MP3 is played whole, whatever its first frame says about its
    /// length: the tag was written before the stream was. Columns: the frames
    /// there are, the frames the tag counts. None at all is MiniMax's tag, and
    /// the demuxer's gapless arithmetic overflows on it; half of them is
    /// Kokoro's, which stopped the sentence half way; the last row is a tag
    /// that tells the truth.
    #[test]
    fn a_streamed_container_is_played_whole_whatever_its_tag_says() {
        use rodio::Source;
        for (frames, claimed) in [(8, 0), (8, 4), (8, 8), (40, 1)] {
            let source = decoder(streamed_mp3(frames, claimed)).expect("an MP3");
            assert_eq!(
                (source.sample_rate().get(), source.channels().get()),
                (44_100, 1),
                "{frames} frames, {claimed} claimed"
            );
            assert_eq!(
                source.count(),
                frames * PER_FRAME,
                "{frames} frames, {claimed} claimed"
            );
        }
    }

    /// What is not audio is an error with a sentence, not a panic and not
    /// silence — and so is a container cut off inside its first frame.
    #[test]
    fn what_cannot_be_decoded_is_an_error() {
        for bytes in [
            b"{\"error\":{\"message\":\"no\"}}".to_vec(),
            vec![0u8; 64],
            streamed_mp3(4, 4)[..10].to_vec(),
        ] {
            let err = decoder(bytes).err().expect("not audio");
            assert!(err.to_string().contains("failed to decode"), "{err:#}");
        }
    }

    /// `rate` Hz mono PCM of silence, `millis` long.
    fn silence(rate: u32, millis: u32) -> AudioClip {
        AudioClip::Pcm {
            sample_rate: rate,
            channels: 1,
            bytes: vec![0; (rate * millis / 1000 * 2) as usize],
        }
    }

    /// A clip's length is what it plays for, whatever its rate and channels —
    /// and a container's is read from its decoded sound.
    #[test]
    fn a_clip_is_as_long_as_it_plays() {
        let length = |clip| {
            Decoded::from_clip(clip)
                .unwrap()
                .map_or(Duration::ZERO, |d| d.length())
        };
        assert_eq!(length(silence(24_000, 1_500)), Duration::from_millis(1_500));
        // Stereo at 44.1 kHz: a second is 88 200 samples.
        let stereo = AudioClip::Pcm {
            sample_rate: 44_100,
            channels: 2,
            bytes: vec![0; 44_100 * 2 * 2],
        };
        assert_eq!(length(stereo), Duration::from_secs(1));
        // An MP3 of 40 frames of 1152 samples at 44.1 kHz, whose tag claims one
        // frame: the sound decides, not the tag.
        let mp3 = length(AudioClip::Encoded(streamed_mp3(40, 1)));
        let expected = Duration::from_secs_f64(40.0 * PER_FRAME as f64 / 44_100.0);
        assert_eq!(mp3, expected);
        // Nothing in it: no clip, zero long.
        assert_eq!(length(AudioClip::Encoded(Vec::new())), Duration::ZERO);
        assert_eq!(length(silence(24_000, 0)), Duration::ZERO);
    }

    /// `ahead` is the sound still queued: every clip held, less how far into
    /// the first one the player is — read here by pulling the samples a sound
    /// card would.
    #[test]
    fn ahead_is_the_sound_still_queued() {
        let (playback, mut card) = Playback::detached();
        let near = |got: Duration, want_ms: u64| {
            let diff = got.as_millis().abs_diff(u128::from(want_ms));
            assert!(diff <= 10, "{got:?}, not ~{want_ms} ms");
        };
        assert_eq!(playback.ahead(), Duration::ZERO);
        // Two seconds at 1 kHz: one sample is a millisecond.
        assert_eq!(
            playback.enqueue(silence(1_000, 1_000)).unwrap(),
            Duration::from_secs(1)
        );
        assert_eq!(
            playback.enqueue(silence(1_000, 1_000)).unwrap(),
            Duration::from_secs(1)
        );
        near(playback.ahead(), 2_000);
        card.by_ref().take(500).for_each(drop);
        near(playback.ahead(), 1_500);
        // Into the second clip: the first is gone from the count.
        card.by_ref().take(1_000).for_each(drop);
        near(playback.ahead(), 500);
        assert!(!playback.is_drained());
        // Past the end: nothing ahead, and the queue is drained.
        card.by_ref().take(600).for_each(drop);
        assert_eq!(playback.ahead(), Duration::ZERO);
        assert!(playback.is_drained());
    }

    /// While paused nothing is played, so nothing ahead is used up — the
    /// pipeline holds its synthesis back by the same count.
    #[test]
    fn a_paused_queue_keeps_what_is_ahead() {
        let (playback, mut card) = Playback::detached();
        playback.enqueue(silence(1_000, 1_000)).unwrap();
        card.by_ref().take(200).for_each(drop);
        let before = playback.ahead();
        playback.pause();
        card.by_ref().take(500).for_each(drop);
        let diff = playback.ahead().as_millis().abs_diff(before.as_millis());
        assert!(diff <= 10, "{before:?} became {:?}", playback.ahead());
    }

    /// A live playback smoke (needs a sound card): synthesize a 440 Hz tone
    /// in the same format the clouds send (PCM s16le 24 kHz mono), queue it,
    /// and wait for it to finish. Checks the riskiest seam — PCM→rodio
    /// (sample rate, signedness, conversion to f32) — and that the queue
    /// actually drains **in real time**, not instantly. The sound (the tone
    /// should be audible for ~0.4s) — a manual check.
    #[test]
    #[ignore = "requires a sound card (a short tone is audible)"]
    fn plays_generated_tone_live() {
        let rate = 24_000u32;
        let secs = 0.4f32;
        let samples = (rate as f32 * secs) as usize;
        let mut bytes = Vec::with_capacity(samples * 2);
        for i in 0..samples {
            let t = i as f32 / rate as f32;
            let amp = (t * 440.0 * std::f32::consts::TAU).sin() * 0.25;
            bytes.extend_from_slice(&((amp * i16::MAX as f32) as i16).to_le_bytes());
        }

        // A headless machine -- a CI runner above all -- has no sound card, and
        // `open` already reports that as a graceful degradation rather than a
        // panic (the app keeps working without audio). Skip on it, as every
        // other smoke skips when its prerequisite is absent; failing would make
        // the remote gate permanently red for an environment fact.
        let Ok(playback) = Playback::open() else {
            eprintln!("skip: no audio device available");
            return;
        };
        let started = std::time::Instant::now();
        let length = playback
            .enqueue(AudioClip::Pcm {
                sample_rate: rate,
                channels: 1,
                bytes,
            })
            .expect("PCM should be accepted by the queue");
        assert_eq!(length, Duration::from_millis(400), "the clip's length");
        assert!(!playback.is_drained(), "the clip is queued");
        while !playback.is_drained() && started.elapsed() < std::time::Duration::from_secs(5) {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let elapsed = started.elapsed();
        assert!(playback.is_drained(), "the queue should drain: {elapsed:?}");
        // Playback runs in real time: neither instant nor endless.
        assert!(
            elapsed >= std::time::Duration::from_millis(300),
            "the clip played too fast ({elapsed:?}) — it likely went nowhere"
        );
        eprintln!("440 Hz tone played in {elapsed:?} (expected ~{secs}s)");
    }

    /// There's no audio in CI/headless — it's important that opening the
    /// device returns an **error, not a panic** (graceful degradation). On a
    /// machine with sound the device opens and the queue starts empty; both
    /// outcomes are acceptable.
    #[test]
    fn open_degrades_gracefully_without_audio_device() {
        match Playback::open() {
            Ok(p) => {
                assert!(p.is_drained(), "a fresh queue is empty");
                assert_eq!(p.ahead(), Duration::ZERO);
                // An empty clip isn't queued and doesn't crash the player.
                assert_eq!(
                    p.enqueue(AudioClip::Encoded(Vec::new())).unwrap(),
                    Duration::ZERO
                );
                p.stop();
            }
            Err(err) => {
                let msg = err.to_string();
                assert!(!msg.is_empty(), "the error should be explainable");
            }
        }
    }
}
