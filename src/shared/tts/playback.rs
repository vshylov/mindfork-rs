//! Воспроизведение синтезированной речи через `rodio` **в процессе приложения**.
//!
//! Почему в процессе, а не сайдкар-плеером: нужна очередь источников с мгновенной
//! отменой, а на Windows пригодного встроенного CLI-плеера нет (см.
//! docs/research/tts.md §6). `Player::append` играет источники последовательно —
//! отсюда бесплатно получается конвейер «синтезируем чанк N+1, пока играет N», а
//! при опустошении очереди наступает тишина до следующего чанка.
//!
//! Две TUI-ловушки, обе учтены здесь:
//! 1. `log_on_drop(false)` — иначе rodio печатает в stderr **поверх TUI**;
//! 2. на Linux сама libasound шумит в stderr при энумерации устройств (cpal#384) —
//!    поэтому устройство открывается **лениво**, только по первой команде `/tts`
//!    (а не на старте приложения), и ошибка открытия — это `Err`, а не паника
//!    (headless/CI/машина без звуковой карты → мягкая деградация, как у
//!    `UnavailableEmbedder`, ADR 0002).

use std::io::Cursor;
use std::num::NonZero;

use anyhow::{Context, Result};
use rodio::{DeviceSinkBuilder, MixerDeviceSink, Player};

use super::AudioClip;

/// Открытое аудио-устройство с очередью воспроизведения.
///
/// Хэндл и плеер `Send + Sync` (cpal 0.17), поэтому живут прямо в фоновой
/// tokio-задаче озвучивания — выделенный аудио-поток не нужен. При дропе
/// устройство закрывается, очередь пропадает вместе с задачей.
pub struct Playback {
    /// Держим устройство живым: при его дропе звук прекращается.
    _sink: MixerDeviceSink,
    player: Player,
}

impl Playback {
    /// Открывает устройство по умолчанию и заводит очередь.
    ///
    /// Возвращает `Err` (а не паникует), если звука нет: headless-окружение, CI,
    /// отсутствующее/занятое устройство. Вызывающий показывает это заметкой.
    pub fn open() -> Result<Self> {
        let mut sink = DeviceSinkBuilder::open_default_sink()
            .context("не удалось открыть аудио-устройство")?;
        // Иначе rodio при дропе пишет в stderr, а его занимает TUI.
        sink.log_on_drop(false);
        let player = Player::connect_new(sink.mixer());
        Ok(Self {
            _sink: sink,
            player,
        })
    }

    /// Ставит клип в конец очереди. Играть он начнёт, когда доиграют предыдущие.
    pub fn enqueue(&self, clip: AudioClip) -> Result<()> {
        match clip {
            AudioClip::Pcm {
                sample_rate,
                channels,
                bytes,
            } => {
                // Сырой PCM идёт мимо декодера: s16le → f32 и прямо в буфер.
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

    /// Сколько клипов ещё в очереди (включая играющий). По нему конвейер
    /// придерживает синтез следующего чанка, чтобы не синтезировать всё вперёд.
    pub fn queued(&self) -> usize {
        self.player.len()
    }

    /// Всё ли проиграно (очередь пуста).
    pub fn is_drained(&self) -> bool {
        self.player.empty()
    }

    /// Немедленно останавливает воспроизведение и очищает очередь.
    pub fn stop(&self) {
        self.player.clear();
    }
}

/// Знаковый 16-битный little-endian PCM → сэмплы `f32` в диапазоне −1.0…1.0
/// (внутреннее представление rodio). Нечётный хвостовой байт отбрасывается.
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
        // 0 → 0.0; i16::MIN → −1.0; 16384 → 0.5. Нечётный хвост отбрасывается.
        let bytes = [0x00, 0x00, 0x00, 0x80, 0x00, 0x40, 0x7f];
        let samples = pcm_s16le_to_f32(&bytes);
        assert_eq!(samples, vec![0.0, -1.0, 0.5]);
        assert!(pcm_s16le_to_f32(&[]).is_empty());
        assert!(pcm_s16le_to_f32(&[0x01]).is_empty());
    }

    /// В CI/headless звука нет — важно, что открытие устройства возвращает
    /// **ошибку, а не панику** (мягкая деградация). На машине со звуком
    /// устройство открывается и очередь стартует пустой; оба исхода допустимы.
    #[test]
    fn open_degrades_gracefully_without_audio_device() {
        match Playback::open() {
            Ok(p) => {
                assert!(p.is_drained(), "новая очередь пуста");
                assert_eq!(p.queued(), 0);
                // Пустой клип не ставится в очередь и не роняет плеер.
                p.enqueue(AudioClip::Encoded(Vec::new())).unwrap();
                p.stop();
            }
            Err(err) => {
                let msg = err.to_string();
                assert!(!msg.is_empty(), "ошибка должна быть объяснимой");
            }
        }
    }
}
