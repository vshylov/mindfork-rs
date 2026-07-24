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

use std::io::Cursor;
use std::num::NonZero;

use anyhow::{Context, Result};
use rodio::{DeviceSinkBuilder, MixerDeviceSink, Player};

use super::AudioClip;

/// An open audio device with a playback queue.
///
/// The handle and the player are `Send + Sync` (cpal 0.17), so they live
/// right inside the speech background tokio task — no dedicated audio thread
/// is needed. On drop the device closes, the queue vanishes along with the task.
pub struct Playback {
    /// Keeps the device alive: dropping it stops the sound.
    _sink: MixerDeviceSink,
    player: Player,
}

impl Playback {
    /// Opens the default device and sets up the queue.
    ///
    /// Returns `Err` (rather than panicking) when there's no audio: a
    /// headless environment, CI, a missing/busy device. The caller shows this as a note.
    pub fn open() -> Result<Self> {
        let mut sink = DeviceSinkBuilder::open_default_sink()
            .context("не удалось открыть аудио-устройство")?;
        // Otherwise rodio writes to stderr on drop, and the TUI occupies it.
        sink.log_on_drop(false);
        let player = Player::connect_new(sink.mixer());
        Ok(Self {
            _sink: sink,
            player,
        })
    }

    /// Puts a clip at the end of the queue. It starts playing once the
    /// previous ones finish.
    pub fn enqueue(&self, clip: AudioClip) -> Result<()> {
        match clip {
            AudioClip::Pcm {
                sample_rate,
                channels,
                bytes,
            } => {
                // Raw PCM bypasses the decoder: s16le → f32 and straight into the buffer.
                let samples = pcm_s16le_to_f32(&bytes);
                if samples.is_empty() {
                    return Ok(());
                }
                let rate = NonZero::new(sample_rate).context("нулевая частота дискретизации")?;
                let ch = NonZero::new(channels).context("нулевое число каналов")?;
                self.player
                    .append(rodio::buffer::SamplesBuffer::new(ch, rate, samples));
            }
            AudioClip::Encoded(bytes) => {
                if bytes.is_empty() {
                    return Ok(());
                }
                let decoder = rodio::Decoder::new(Cursor::new(bytes))
                    .context("не удалось разобрать аудио от TTS-сервера")?;
                self.player.append(decoder);
            }
        }
        Ok(())
    }

    /// How many clips are still queued (including the one playing). The
    /// pipeline uses this to hold back synthesizing the next chunk, so it
    /// doesn't synthesize everything ahead of time.
    pub fn queued(&self) -> usize {
        self.player.len()
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

        let playback = Playback::open().expect("a sound card is required");
        let started = std::time::Instant::now();
        playback
            .enqueue(AudioClip::Pcm {
                sample_rate: rate,
                channels: 1,
                bytes,
            })
            .expect("PCM should be accepted by the queue");
        assert_eq!(playback.queued(), 1, "the clip is queued");
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
                assert_eq!(p.queued(), 0);
                // An empty clip isn't queued and doesn't crash the player.
                p.enqueue(AudioClip::Encoded(Vec::new())).unwrap();
                p.stop();
            }
            Err(err) => {
                let msg = err.to_string();
                assert!(!msg.is_empty(), "the error should be explainable");
            }
        }
    }
}
