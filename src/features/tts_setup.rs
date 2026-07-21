//! Провизия локального озвучивания (`mindfork tts setup`, этап 2 направления TTS —
//! [docs/research/tts.md](../../docs/research/tts.md) §4, ADR 0009). Скачивает в
//! `data/tts/`: бинарь **piper** (платформенный архив с GitHub) и **голоса**
//! (`*.onnx` + `*.onnx.json` с HuggingFace `rhasspy/piper-voices`) — всё по
//! **lock-списку с точными URL + sha256** (устойчиво к «latest»; у релиза piper
//! 2023.11.14-2 GitHub не отдаёт `digest`, поэтому хэши посчитаны при заведении
//! списка). Провизия идемпотентна; общая механика скачивания/сверки/распаковки —
//! [`crate::features::provision`].
//!
//! Раскладка совпадает с тем, что ищет рантайм ([`crate::shared::tts::sidecar`]):
//! `data/tts/piper/piper[.exe]` (+ `espeak-ng-data/` рядом) и
//! `data/tts/voices/<имя>.onnx(.json)`.
//!
//! **Лицензии голосов** (решение Р2): берём только permissive — `ru_RU-dmitri`
//! и `ru_RU-denis` (данные **CC0**) и английский `en_US-amy` (CC0/MIT-совместимый
//! датасет). `ru_RU-irina` («License: Unknown») и `ru_RU-ruslan` (CC BY-NC-SA)
//! **не включены**.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::features::provision::{
    download_bytes, download_to_file, extract_targz, extract_zip, http_client,
};
use crate::shared::i18n::Locale;
use crate::shared::tts::sidecar::locate_piper;

/// Релиз piper, к которому привязан lock-список (последний MIT-релиз; апстрим
/// заархивирован — версия зафиксирована сознательно).
pub const PIPER_VERSION: &str = "2023.11.14-2";
/// User-Agent для скачиваний (GitHub/HuggingFace отвергают пустой UA).
const USER_AGENT: &str = "mindfork-rs-tts-setup";
/// Подкаталог с голосами.
const VOICES_SUBDIR: &str = "voices";
/// Подкаталог с распакованным дистрибутивом piper.
const PIPER_SUBDIR: &str = "piper";

/// Платформенный архив piper (zip на Windows, tar.gz на прочих).
struct PlatformArchive {
    os: &'static str,
    arch: &'static str,
    url: &'static str,
    sha256: &'static str,
    /// Zip (Windows) или tar.gz (остальные).
    zip: bool,
}

/// Lock-список архивов piper 2023.11.14-2 (sha256 посчитаны при заведении списка).
const ARCHIVES: &[PlatformArchive] = &[
    PlatformArchive {
        os: "windows",
        arch: "x86_64",
        url: "https://github.com/rhasspy/piper/releases/download/2023.11.14-2/piper_windows_amd64.zip",
        sha256: "f3c58906402b24f3a96d92145f58acba6d86c9b5db896d207f78dc80811efcea",
        zip: true,
    },
    PlatformArchive {
        os: "linux",
        arch: "x86_64",
        url: "https://github.com/rhasspy/piper/releases/download/2023.11.14-2/piper_linux_x86_64.tar.gz",
        sha256: "a50cb45f355b7af1f6d758c1b360717877ba0a398cc8cbe6d2a7a3a26e225992",
        zip: false,
    },
    PlatformArchive {
        os: "linux",
        arch: "aarch64",
        url: "https://github.com/rhasspy/piper/releases/download/2023.11.14-2/piper_linux_aarch64.tar.gz",
        sha256: "fea0fd2d87c54dbc7078d0f878289f404bd4d6eea6e7444a77835d1537ab88eb",
        zip: false,
    },
    PlatformArchive {
        os: "macos",
        arch: "aarch64",
        url: "https://github.com/rhasspy/piper/releases/download/2023.11.14-2/piper_macos_aarch64.tar.gz",
        sha256: "6b1eb03b3735946cb35216e063e7eebcc33a6bbf5dd96ec0217959bf1cdcb0cc",
        zip: false,
    },
];

/// Голос piper: пара «модель + конфиг». `name` — имя файла без расширения (оно же
/// значение настройки «Голос»).
#[derive(Debug)]
struct Voice {
    name: &'static str,
    onnx_url: &'static str,
    onnx_sha256: &'static str,
    json_url: &'static str,
    json_sha256: &'static str,
    /// Ставится по умолчанию (иначе — только по `--voice`).
    default: bool,
}

/// Lock-список голосов. По умолчанию ставятся русский `dmitri` (голос настроек по
/// умолчанию) и английский `amy`; `denis` — запасной русский, ставится явным
/// `--voice ru_RU-denis-medium`. Все — medium-качество, 22.05 кГц, ~63 МБ.
const VOICES: &[Voice] = &[
    Voice {
        name: "ru_RU-dmitri-medium",
        onnx_url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/ru/ru_RU/dmitri/medium/ru_RU-dmitri-medium.onnx",
        onnx_sha256: "f073356ebc4bd0f80c5af58df2953a5988bd5bdab1eb38635ce960b071fbefcb",
        json_url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/ru/ru_RU/dmitri/medium/ru_RU-dmitri-medium.onnx.json",
        json_sha256: "667ef3117bc642c2892dff7690d8bdc8ca4228aeaa783b2dc1416df632855e0d",
        default: true,
    },
    Voice {
        name: "en_US-amy-medium",
        onnx_url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/amy/medium/en_US-amy-medium.onnx",
        onnx_sha256: "b3a6e47b57b8c7fbe6a0ce2518161a50f59a9cdd8a50835c02cb02bdd6206c18",
        json_url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/amy/medium/en_US-amy-medium.onnx.json",
        json_sha256: "95a23eb4d42909d38df73bb9ac7f45f597dbfcde2d1bf9526fdeaf5466977d77",
        default: true,
    },
    Voice {
        name: "ru_RU-denis-medium",
        onnx_url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/ru/ru_RU/denis/medium/ru_RU-denis-medium.onnx",
        onnx_sha256: "15fab56e11a097858ee115545d0f697fc2a316c41a291a5362349fb870411b0a",
        json_url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/ru/ru_RU/denis/medium/ru_RU-denis-medium.onnx.json",
        json_sha256: "831c860dac0b5073eaa81610a0a638ec23d90a6cf8e5f871b4485c2cec3767c8",
        default: false,
    },
];

/// Параметры провизии озвучивания.
#[derive(Debug, Clone, Default)]
pub struct SetupOptions {
    /// Перекачать/переустановить всё, даже если уже на месте.
    pub force: bool,
    /// Дополнительные голоса (сверх устанавливаемых по умолчанию).
    pub voices: Vec<String>,
}

/// Провизия локального озвучивания в каталог `dir` (`data/tts/`). `progress` —
/// колбэк строк для stdout. Идемпотентно: уже установленное пропускается (кроме
/// `force`).
pub async fn setup(
    dir: &Path,
    opts: &SetupOptions,
    loc: &Locale,
    mut progress: impl FnMut(&str),
) -> Result<()> {
    std::fs::create_dir_all(dir)
        .with_context(|| loc.tf("tts.setup.mkdir", &[("path", &dir.display().to_string())]))?;
    let client = http_client(USER_AGENT, loc)?;

    ensure_piper(&client, dir, opts, loc, &mut progress).await?;
    let wanted = selected_voices(&opts.voices, loc)?;
    for voice in wanted {
        ensure_voice(&client, dir, voice, opts, loc, &mut progress).await?;
    }

    progress(loc.t("tts.setup.done"));
    Ok(())
}

/// Архив piper для платформы `(os, arch)` (чистая, тестируемая).
fn archive_for(os: &str, arch: &str) -> Option<&'static PlatformArchive> {
    ARCHIVES.iter().find(|a| a.os == os && a.arch == arch)
}

/// Голоса к установке: дефолтные + явно запрошенные. Неизвестное имя — ошибка со
/// списком доступных (молча пропустить хуже: пользователь ждал бы голос).
fn selected_voices(extra: &[String], loc: &Locale) -> Result<Vec<&'static Voice>> {
    let mut out: Vec<&'static Voice> = VOICES.iter().filter(|v| v.default).collect();
    for name in extra {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let Some(v) = VOICES.iter().find(|v| v.name.eq_ignore_ascii_case(name)) else {
            let known: Vec<&str> = VOICES.iter().map(|v| v.name).collect();
            bail!(
                "{}",
                loc.tf(
                    "tts.setup.voice.unknown",
                    &[("name", name), ("known", &known.join(", "))],
                )
            );
        };
        if !out.iter().any(|x| x.name == v.name) {
            out.push(v);
        }
    }
    Ok(out)
}

/// Гарантирует наличие бинаря `piper` в `dir`; возвращает путь к нему.
async fn ensure_piper(
    client: &reqwest::Client,
    dir: &Path,
    opts: &SetupOptions,
    loc: &Locale,
    progress: &mut impl FnMut(&str),
) -> Result<PathBuf> {
    if !opts.force
        && let Some(found) = locate_piper(Some(dir), None)
    {
        progress(&loc.tf(
            "tts.setup.piper.present",
            &[("path", &found.display().to_string())],
        ));
        return Ok(found);
    }
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    let Some(arch_info) = archive_for(os, arch) else {
        bail!(
            "{}",
            loc.tf("tts.setup.piper.no_platform", &[("os", os), ("arch", arch)])
        );
    };
    progress(&loc.tf(
        "tts.setup.piper.downloading",
        &[("version", PIPER_VERSION), ("os", os), ("arch", arch)],
    ));
    let ext = if arch_info.zip { "zip" } else { "tar.gz" };
    let archive_path = dir.join(format!("piper-dist.{ext}"));
    download_to_file(
        client,
        arch_info.url,
        &archive_path,
        arch_info.sha256,
        loc,
        progress,
    )
    .await?;

    progress(loc.t("tts.setup.piper.extracting"));
    // Оба архива piper несут верхний каталог `piper/` — распаковываем в корень
    // `data/tts/`, чтобы получилось ровно `data/tts/piper/piper[.exe]`.
    if arch_info.zip {
        extract_zip(&archive_path, dir, loc)
    } else {
        extract_targz(&archive_path, dir, loc)
    }
    .with_context(|| loc.t("tts.setup.piper.extract_ctx").to_string())?;
    let _ = std::fs::remove_file(&archive_path);

    let found = locate_piper(Some(dir), None).ok_or_else(|| {
        anyhow::anyhow!(
            "{}",
            loc.tf(
                "tts.setup.piper.not_found",
                &[("path", &dir.join(PIPER_SUBDIR).display().to_string())],
            )
        )
    })?;
    // На unix архив несёт права исполнения; отдельного chmod не требуется.
    Ok(found)
}

/// Скачивает голос (модель + конфиг) в `<dir>/voices/`.
async fn ensure_voice(
    client: &reqwest::Client,
    dir: &Path,
    voice: &Voice,
    opts: &SetupOptions,
    loc: &Locale,
    progress: &mut impl FnMut(&str),
) -> Result<()> {
    let voices = dir.join(VOICES_SUBDIR);
    std::fs::create_dir_all(&voices).with_context(|| loc.t("tts.setup.voice.mkdir").to_string())?;
    let onnx = voices.join(format!("{}.onnx", voice.name));
    let json = voices.join(format!("{}.onnx.json", voice.name));
    if !opts.force && onnx.is_file() && json.is_file() {
        progress(&loc.tf("tts.setup.voice.present", &[("name", voice.name)]));
        return Ok(());
    }
    progress(&loc.tf("tts.setup.voice.downloading", &[("name", voice.name)]));
    download_to_file(
        client,
        voice.onnx_url,
        &onnx,
        voice.onnx_sha256,
        loc,
        progress,
    )
    .await?;
    // Конфиг маленький — целиком в память.
    let cfg = download_bytes(client, voice.json_url, voice.json_sha256, loc).await?;
    std::fs::write(&json, &cfg)
        .with_context(|| loc.tf("tts.setup.voice.write", &[("name", voice.name)]))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::i18n::{Lang, locale};

    fn ru() -> &'static Locale {
        locale(Lang::Ru)
    }

    #[test]
    fn archive_selection_by_platform() {
        assert!(archive_for("windows", "x86_64").is_some_and(|a| a.zip));
        assert!(archive_for("linux", "x86_64").is_some_and(|a| !a.zip));
        assert!(archive_for("linux", "aarch64").is_some());
        assert!(archive_for("macos", "aarch64").is_some());
        // Неизвестная платформа — нет автоскачивания (понятная ошибка выше).
        assert!(archive_for("plan9", "x86_64").is_none());
        assert!(archive_for("windows", "riscv64").is_none());
    }

    #[test]
    fn lockfile_is_pinned_and_permissive() {
        for a in ARCHIVES {
            assert!(a.url.starts_with("https://"), "{}", a.url);
            assert!(
                a.url.contains(PIPER_VERSION),
                "версия пришпилена: {}",
                a.url
            );
            assert_eq!(a.sha256.len(), 64, "{}", a.url);
        }
        for v in VOICES {
            assert!(v.onnx_url.starts_with("https://"), "{}", v.name);
            assert_eq!(v.onnx_sha256.len(), 64, "{}", v.name);
            assert_eq!(v.json_sha256.len(), 64, "{}", v.name);
        }
        // NC-лицензированные голоса в набор не попадают (решение Р2).
        let names: Vec<&str> = VOICES.iter().map(|v| v.name).collect();
        assert!(!names.iter().any(|n| n.contains("ruslan")), "{names:?}");
        assert!(!names.iter().any(|n| n.contains("irina")), "{names:?}");
    }

    #[test]
    fn default_voices_cover_config_default_and_english() {
        let picked = selected_voices(&[], ru()).unwrap();
        let names: Vec<&str> = picked.iter().map(|v| v.name).collect();
        // Голос из дефолтного конфига обязан устанавливаться «из коробки».
        assert!(
            names.contains(&crate::shared::config::DEFAULT_TTS_MANAGED_VOICE),
            "{names:?}"
        );
        assert!(names.iter().any(|n| n.starts_with("en_US")), "{names:?}");
        // Запасной русский — только по явному запросу, без дублей.
        assert!(!names.contains(&"ru_RU-denis-medium"));
        let picked = selected_voices(&["ru_RU-denis-medium".into()], ru()).unwrap();
        assert_eq!(picked.len(), 3);
        let twice = selected_voices(
            &["ru_RU-dmitri-medium".into(), "ru_RU-dmitri-medium".into()],
            ru(),
        )
        .unwrap();
        assert_eq!(twice.len(), 2, "дубли не плодятся");
    }

    #[test]
    fn unknown_voice_is_an_error_listing_known_ones() {
        let err = selected_voices(&["ru_RU-ruslan-medium".into()], ru())
            .unwrap_err()
            .to_string();
        assert!(err.contains("ru_RU-dmitri-medium"), "{err}");
        // Регрессия против забытого `loc`: en-текст без кириллицы.
        let en = selected_voices(&["no-such-voice".into()], locale(Lang::En))
            .unwrap_err()
            .to_string();
        assert!(!en.chars().any(|c| ('а'..='я').contains(&c)), "{en}");
    }
}
