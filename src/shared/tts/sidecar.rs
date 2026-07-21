//! Локальный движок озвучивания — **сайдкар `piper`** (MIT-релиз 2023.11.14-2),
//! managed-режим TTS. Бинарь и голоса лежат рядом с приложением (`data/tts/`),
//! наполняет их команда `mindfork tts setup` (провизия по lock-списку URL+sha256,
//! паттерн ADR 0005). См. docs/research/tts.md §4 и ADR 0009.
//!
//! **Почему piper, а не CLI sherpa-onnx** (развилка Р2, подварианты (а)/(в)):
//! спайк этапа 2 показал, что `sherpa-onnx-offline-tts` принимает текст **только
//! позиционным аргументом**, а его узкий `main()` заставляет Windows-CRT
//! конвертировать argv из UTF-16 в ANSI-кодовую страницу (у русской локали обычно
//! 1252) — вся кириллица превращается в `?` и синтезируется тишина; внешний
//! манифест с `activeCodePage=UTF-8` игнорируется, опции «текст из файла» у CLI
//! нет. Piper принимает текст **через stdin** (UTF-8 байтами, под нашим контролем)
//! и отдаёт сырой PCM в stdout (`--output_raw`) — временный файл не нужен.
//!
//! Цена варианта: апстрим заархивирован (10.2025) — багфиксов не будет; модель
//! грузится на каждый спавн (~0.8 с, замерено спайком), поэтому у движка большой
//! [`TtsEngine::max_input_chars`] — чанкер складывает больше предложений в один
//! вызов. Голоса нужны **оригинальные** (HuggingFace `rhasspy/piper-voices`):
//! переэкспортированные ONNX из model zoo sherpa роняют бинарь.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, Result};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

use super::{AudioClip, TtsEngine, TtsSetupError};

/// Имя бинаря `piper` по платформе.
const PIPER_BIN: &str = if cfg!(windows) { "piper.exe" } else { "piper" };
/// Переменная окружения: путь к бинарю `piper` (override поиска; ручная установка,
/// смоуки).
const ENV_PIPER: &str = "MINDFORK_TTS_PIPER";
/// Подкаталог с голосами внутри каталога озвучивания.
const VOICES_SUBDIR: &str = "voices";
/// Частота голоса, если её не удалось прочитать из `.onnx.json` (у piper-голосов
/// medium-качества это 22050 Гц).
const FALLBACK_SAMPLE_RATE: u32 = 22050;
/// Потолок текста одного вызова. У локального движка нет лимита провайдера, но
/// каждый вызов — новый процесс с загрузкой модели, поэтому чанки укрупняем.
const MAX_INPUT_CHARS: usize = 2000;

/// Локальный синтез речи бинарём `piper`.
pub struct PiperTts {
    binary: PathBuf,
    /// Модель голоса (`*.onnx`); конфиг `*.onnx.json` лежит рядом.
    voice: PathBuf,
    /// Каталог данных espeak-ng (рядом с бинарём) — фонемизатор.
    espeak_data: Option<PathBuf>,
    /// Частота сэмплов голоса (из `.onnx.json`).
    sample_rate: u32,
    /// Длительность фонемы: у piper больше = медленнее, поэтому это `1/speed`.
    length_scale: f32,
}

impl PiperTts {
    /// Собирает движок из настроек: ищет бинарь и голос, читает частоту голоса.
    ///
    /// `dir` — каталог озвучивания (`data/tts/`), `voice` — имя голоса
    /// (файл в `<dir>/voices/`) либо полный путь к `*.onnx`, `speed` — скорость
    /// (больше = быстрее).
    pub fn new(
        dir: Option<&Path>,
        binary_override: Option<&str>,
        voice: Option<&str>,
        speed: f32,
    ) -> std::result::Result<Self, TtsSetupError> {
        let binary = resolve_binary(dir, binary_override).ok_or(TtsSetupError::Binary)?;
        let voice = resolve_voice(dir, voice).ok_or(TtsSetupError::Voice)?;
        let sample_rate = read_sample_rate(&voice).unwrap_or(FALLBACK_SAMPLE_RATE);
        let espeak_data = binary
            .parent()
            .map(|d| d.join("espeak-ng-data"))
            .filter(|d| d.is_dir());
        Ok(Self {
            binary,
            voice,
            espeak_data,
            sample_rate,
            // Скорость 0 или отрицательная бессмысленна — считаем её «обычной».
            length_scale: if speed > 0.0 { 1.0 / speed } else { 1.0 },
        })
    }

    /// Аргументы запуска (чистая, тестируемая часть).
    fn args(&self) -> Vec<String> {
        let mut args = vec![
            "--model".into(),
            self.voice.to_string_lossy().into_owned(),
            "--config".into(),
            config_path(&self.voice).to_string_lossy().into_owned(),
            // Сырой PCM в stdout — по мере генерации, без временного файла.
            "--output_raw".into(),
            "--length_scale".into(),
            format!("{:.3}", self.length_scale),
        ];
        if let Some(data) = &self.espeak_data {
            args.push("--espeak_data".into());
            args.push(data.to_string_lossy().into_owned());
        }
        args
    }
}

#[async_trait::async_trait]
impl TtsEngine for PiperTts {
    async fn synthesize(&self, text: &str, cancel: &CancellationToken) -> Result<AudioClip> {
        // Piper читает stdin построчно: одна строка = одна реплика. Переводы строк
        // внутри чанка схлопываем, чтобы реплика осталась одной (иначе бинарь
        // синтезировал бы её кусками с лишними паузами).
        let line = text.replace(['\n', '\r'], " ");
        let mut child = tokio::process::Command::new(&self.binary)
            .args(self.args())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Логи piper идут в stderr; в файл их не тянем (stdout занят звуком),
            // но и не блокируем — пусть уходят в никуда.
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("не удалось запустить {}", self.binary.display()))?;

        if let Some(mut stdin) = child.stdin.take() {
            // Текст уходит **байтами UTF-8** — здесь нет конвертации в кодовую
            // страницу, которая ломает кириллицу в argv на Windows.
            let payload = format!("{line}\n");
            let _ = stdin.write_all(payload.as_bytes()).await;
            // Закрываем stdin — иначе piper ждёт следующую строку и не завершится.
            drop(stdin);
        }

        let bytes = tokio::select! {
            out = child.wait_with_output() => {
                let out = out.context("ошибка ожидания piper")?;
                if !out.status.success() {
                    anyhow::bail!("piper завершился с кодом {:?}", out.status.code());
                }
                out.stdout
            }
            // Отмена: фьюча дропается вместе с `child` (kill_on_drop).
            _ = cancel.cancelled() => anyhow::bail!("озвучивание отменено"),
        };

        Ok(AudioClip::Pcm {
            sample_rate: self.sample_rate,
            channels: 1,
            bytes,
        })
    }

    fn max_input_chars(&self) -> usize {
        MAX_INPUT_CHARS
    }
}

/// Ищет бинарь `piper`: явный override из настроек → env-переменная → раскладка
/// каталога озвучивания (`<dir>/piper/piper[.exe]` после `tts setup`, либо
/// `<dir>/piper[.exe]` при ручной установке).
pub fn locate_piper(dir: Option<&Path>, binary_override: Option<&str>) -> Option<PathBuf> {
    resolve_binary(dir, binary_override)
}

fn resolve_binary(dir: Option<&Path>, binary_override: Option<&str>) -> Option<PathBuf> {
    if let Some(p) = binary_override.map(str::trim).filter(|p| !p.is_empty()) {
        let path = PathBuf::from(p);
        return path.is_file().then_some(path);
    }
    if let Some(p) = std::env::var_os(ENV_PIPER).filter(|v| !v.is_empty()) {
        let path = PathBuf::from(p);
        return path.is_file().then_some(path);
    }
    let dir = dir?;
    let dist = dir.join("piper").join(PIPER_BIN);
    if dist.is_file() {
        return Some(dist);
    }
    let direct = dir.join(PIPER_BIN);
    direct.is_file().then_some(direct)
}

/// Резолвит голос: полный путь к `*.onnx` либо имя файла в `<dir>/voices/`.
/// Рядом обязан лежать `*.onnx.json` (без него piper не запустится).
fn resolve_voice(dir: Option<&Path>, voice: Option<&str>) -> Option<PathBuf> {
    let name = voice.map(str::trim).filter(|v| !v.is_empty())?;
    let as_path = PathBuf::from(name);
    if as_path.is_file() && config_path(&as_path).is_file() {
        return Some(as_path);
    }
    let dir = dir?;
    let file = if name.ends_with(".onnx") {
        name.to_string()
    } else {
        format!("{name}.onnx")
    };
    let path = dir.join(VOICES_SUBDIR).join(file);
    (path.is_file() && config_path(&path).is_file()).then_some(path)
}

/// Путь к конфигу голоса: `<voice>.onnx` → `<voice>.onnx.json`.
pub(crate) fn config_path(voice: &Path) -> PathBuf {
    let mut s = voice.as_os_str().to_os_string();
    s.push(".json");
    PathBuf::from(s)
}

/// Читает частоту сэмплов из конфига голоса (`audio.sample_rate`).
fn read_sample_rate(voice: &Path) -> Option<u32> {
    let raw = std::fs::read_to_string(config_path(voice)).ok()?;
    let json: serde_json::Value = serde_json::from_str(&raw).ok()?;
    json.get("audio")?
        .get("sample_rate")?
        .as_u64()
        .map(|v| v as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Готовит каталог озвучивания с фиктивным бинарём и голосом.
    fn fake_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("piper");
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join(PIPER_BIN), b"stub").unwrap();
        fs::create_dir_all(bin.join("espeak-ng-data")).unwrap();
        let voices = dir.path().join(VOICES_SUBDIR);
        fs::create_dir_all(&voices).unwrap();
        fs::write(voices.join("ru_RU-dmitri-medium.onnx"), b"stub").unwrap();
        fs::write(
            voices.join("ru_RU-dmitri-medium.onnx.json"),
            br#"{"audio":{"sample_rate":22050}}"#,
        )
        .unwrap();
        dir
    }

    #[test]
    fn locates_binary_and_voice_by_name() {
        let dir = fake_dir();
        let tts = PiperTts::new(Some(dir.path()), None, Some("ru_RU-dmitri-medium"), 1.0).unwrap();
        assert_eq!(tts.sample_rate, 22050);
        assert!(tts.espeak_data.is_some(), "espeak-ng-data рядом с бинарём");
        // Имя голоса можно писать и с расширением.
        assert!(
            PiperTts::new(
                Some(dir.path()),
                None,
                Some("ru_RU-dmitri-medium.onnx"),
                1.0
            )
            .is_ok()
        );
    }

    #[test]
    fn missing_binary_and_voice_are_structural_errors() {
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(
            PiperTts::new(Some(empty.path()), None, Some("ru_RU-dmitri-medium"), 1.0).err(),
            Some(TtsSetupError::Binary),
            "без бинаря — подсказка выполнить `tts setup`"
        );
        let dir = fake_dir();
        assert_eq!(
            PiperTts::new(Some(dir.path()), None, Some("нет-такого-голоса"), 1.0).err(),
            Some(TtsSetupError::Voice)
        );
        // Голос без конфига `.onnx.json` не годится (piper с ним не запустится).
        let lone = dir.path().join(VOICES_SUBDIR).join("lone.onnx");
        fs::write(&lone, b"stub").unwrap();
        assert_eq!(
            PiperTts::new(Some(dir.path()), None, Some("lone"), 1.0).err(),
            Some(TtsSetupError::Voice)
        );
    }

    #[test]
    fn args_carry_voice_config_raw_output_and_inverted_speed() {
        let dir = fake_dir();
        let tts = PiperTts::new(Some(dir.path()), None, Some("ru_RU-dmitri-medium"), 2.0).unwrap();
        let args = tts.args();
        assert!(args.iter().any(|a| a == "--output_raw"), "{args:?}");
        assert!(args.iter().any(|a| a.ends_with(".onnx")), "{args:?}");
        assert!(args.iter().any(|a| a.ends_with(".onnx.json")), "{args:?}");
        // speed=2 (вдвое быстрее) → length_scale=0.5 (у piper больше = медленнее).
        let idx = args.iter().position(|a| a == "--length_scale").unwrap();
        assert_eq!(args[idx + 1], "0.500");
        assert!(args.iter().any(|a| a == "--espeak_data"), "{args:?}");
    }

    #[test]
    fn speed_zero_falls_back_to_normal_pace() {
        let dir = fake_dir();
        let tts = PiperTts::new(Some(dir.path()), None, Some("ru_RU-dmitri-medium"), 0.0).unwrap();
        assert_eq!(tts.length_scale, 1.0);
    }

    #[test]
    fn explicit_binary_override_wins() {
        let dir = fake_dir();
        let other = dir.path().join("custom-piper");
        fs::write(&other, b"stub").unwrap();
        let tts = PiperTts::new(
            Some(dir.path()),
            Some(other.to_str().unwrap()),
            Some("ru_RU-dmitri-medium"),
            1.0,
        )
        .unwrap();
        assert_eq!(tts.binary, other);
        // Указан несуществующий путь — это ошибка, а не тихий откат к поиску.
        assert_eq!(
            PiperTts::new(
                Some(dir.path()),
                Some("нет/такого/piper"),
                Some("ru_RU-dmitri-medium"),
                1.0
            )
            .err(),
            Some(TtsSetupError::Binary)
        );
    }

    #[test]
    fn sample_rate_falls_back_when_config_is_unreadable() {
        let dir = tempfile::tempdir().unwrap();
        let voices = dir.path().join(VOICES_SUBDIR);
        fs::create_dir_all(&voices).unwrap();
        fs::write(voices.join("v.onnx"), b"stub").unwrap();
        fs::write(voices.join("v.onnx.json"), b"{ not json }").unwrap();
        fs::write(dir.path().join(PIPER_BIN), b"stub").unwrap();
        let tts = PiperTts::new(Some(dir.path()), None, Some("v"), 1.0).unwrap();
        assert_eq!(tts.sample_rate, FALLBACK_SAMPLE_RATE);
    }

    /// Живой смоук: реальный бинарь и голос из `data/tts/` (после `tts setup`).
    /// Запуск: `MINDFORK_TTS_DIR=<путь> cargo test --ignored`.
    #[tokio::test]
    #[ignore = "нужен установленный piper (mindfork tts setup)"]
    async fn synthesizes_russian_speech() {
        let Some(dir) = std::env::var_os("MINDFORK_TTS_DIR") else {
            eprintln!("MINDFORK_TTS_DIR не задан — пропуск");
            return;
        };
        let dir = PathBuf::from(dir);
        let tts = PiperTts::new(Some(&dir), None, Some("ru_RU-dmitri-medium"), 1.0)
            .expect("piper и голос должны быть установлены");
        let clip = tts
            .synthesize(
                "Привет! Это проверка локальной озвучки на русском языке.",
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        let AudioClip::Pcm {
            sample_rate,
            channels,
            bytes,
        } = clip
        else {
            panic!("сайдкар отдаёт сырой PCM");
        };
        assert_eq!(sample_rate, 22050);
        assert_eq!(channels, 1);
        let seconds = bytes.len() as f32 / 2.0 / sample_rate as f32;
        eprintln!("синтезировано {seconds:.2} с речи ({} байт)", bytes.len());
        assert!(seconds > 1.0, "кириллица не должна съедаться: {seconds} с");
    }

    /// Живой смоук всей цепочки локального движка: синтез + воспроизведение
    /// (слышна русская фраза с латинскими вставками — главный риск piper).
    #[tokio::test]
    #[ignore = "нужны piper и звуковая карта (слышна речь)"]
    async fn speaks_aloud_through_playback() {
        let Some(dir) = std::env::var_os("MINDFORK_TTS_DIR") else {
            eprintln!("MINDFORK_TTS_DIR не задан — пропуск");
            return;
        };
        let tts = PiperTts::new(
            Some(&PathBuf::from(dir)),
            None,
            Some("ru_RU-dmitri-medium"),
            1.0,
        )
        .expect("piper и голос должны быть установлены");
        let clip = tts
            .synthesize(
                "Модель отвечает через llama-server по протоколу OpenAI.",
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        let playback = super::super::playback::Playback::open().expect("аудио-устройство");
        playback.enqueue(clip).unwrap();
        while !playback.is_drained() {
            tokio::time::sleep(std::time::Duration::from_millis(80)).await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    /// Отмена во время синтеза не оставляет процесс: `kill_on_drop` убивает piper.
    #[tokio::test]
    #[ignore = "нужен установленный piper (mindfork tts setup)"]
    async fn cancellation_aborts_synthesis() {
        let Some(dir) = std::env::var_os("MINDFORK_TTS_DIR") else {
            eprintln!("MINDFORK_TTS_DIR не задан — пропуск");
            return;
        };
        let tts = PiperTts::new(
            Some(&PathBuf::from(dir)),
            None,
            Some("ru_RU-dmitri-medium"),
            1.0,
        )
        .expect("piper и голос должны быть установлены");
        let cancel = CancellationToken::new();
        cancel.cancel();
        let err = tts
            .synthesize("Длинная фраза, которую мы отменяем.", &cancel)
            .await
            .unwrap_err();
        eprintln!("отмена: {err}");
    }
}
