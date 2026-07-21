//! Провизия ассетов песочницы Python (`mindfork sandbox setup`, Фаза 2 —
//! [docs/research/python-wasmer-sandbox.md](../../docs/research/python-wasmer-sandbox.md)).
//! Скачивает в `data/sandbox/`: бинарь `wasmer` (платформенный tar.gz с GitHub),
//! `python.webc` (через сам `wasmer`), колёса пакетов (numpy с wasix-индекса,
//! requests-стек с PyPI) — всё по **lock-списку с точными URL + sha256** (устойчиво
//! к «latest»). Провизия идемпотентна; сетевой слой тонкий, логика — чистая/тестируемая.
//!
//! Слой `features`: без TUI (команда CLI печатает прогресс в stdout). Расположение
//! бинаря после распаковки совпадает с тем, что ищет `shared::sandbox` (общий
//! резолвер [`crate::shared::sandbox::locate_wasmer`]).

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::features::provision::{
    download_bytes, download_to_file, extract_targz, http_client, unpack_zip,
};
use crate::shared::i18n::Locale;
use crate::shared::sandbox::{SandboxRunner, WasmerSandbox, locate_wasmer};

/// Версия `wasmer`, к которой привязан lock-список (GitHub release tag `v<...>`).
pub const WASMER_VERSION: &str = "7.2.0";
/// Пакет CPython в реестре Wasmer (скачивается в `python.webc`).
const PYTHON_PACKAGE: &str = "python/python";
/// User-Agent для скачиваний (GitHub/PyPI иногда отвергают пустой UA).
const USER_AGENT: &str = "mindfork-rs-sandbox-setup";

/// Платформенный архив `wasmer` с GitHub (tar.gz). `os`/`arch` — из
/// [`std::env::consts`]. Распаковывается целиком в `<dir>/wasmer-dist/`.
struct PlatformArchive {
    os: &'static str,
    arch: &'static str,
    url: &'static str,
    sha256: &'static str,
}

/// Lock-список архивов `wasmer` v7.2.0 (sha256 — из GitHub release assets `digest`).
const ARCHIVES: &[PlatformArchive] = &[
    PlatformArchive {
        os: "windows",
        arch: "x86_64",
        url: "https://github.com/wasmerio/wasmer/releases/download/v7.2.0/wasmer-windows-amd64.tar.gz",
        sha256: "140cc0085f97f2b0150c6803051fad4fe379d7b79ade819283e1f174d0148c48",
    },
    PlatformArchive {
        os: "linux",
        arch: "x86_64",
        url: "https://github.com/wasmerio/wasmer/releases/download/v7.2.0/wasmer-linux-amd64.tar.gz",
        sha256: "fce71a4b0d504b9925e2461d1368b24cce60001111edb3fa871df8187a8a40f2",
    },
    PlatformArchive {
        os: "linux",
        arch: "aarch64",
        url: "https://github.com/wasmerio/wasmer/releases/download/v7.2.0/wasmer-linux-aarch64.tar.gz",
        sha256: "e0033919790592b38410ea38e9b327f3c17ebb355e40f3f6db5191bad5ec3b31",
    },
    PlatformArchive {
        os: "macos",
        arch: "aarch64",
        url: "https://github.com/wasmerio/wasmer/releases/download/v7.2.0/wasmer-darwin-arm64.tar.gz",
        sha256: "b1f087a7472d30e4a61cd7918925a69daacf304b50d67d511326b09e1847b2a4",
    },
];

/// Колесо Python-пакета (`.whl` = zip). `dir` — имя каталога пакета в `site-packages`
/// (для идемпотентности: есть каталог → колесо уже распаковано).
struct Wheel {
    dir: &'static str,
    url: &'static str,
    sha256: &'static str,
}

/// Lock-список колёс. **numpy**/**pandas** — нативные wasix-колёса с
/// `pythonindex.wasix.org`; их чистые зависимости (dateutil/six/pytz/tzdata) и
/// **requests-стек** (requests/urllib3/certifi/idna/charset_normalizer) — чистый
/// Python с PyPI (`py3-none-any`). Версии закреплены; sha256 — из индекса/PyPI.
const WHEELS: &[Wheel] = &[
    Wheel {
        dir: "numpy",
        url: "https://pythonindex.wasix.org/packages/numpy-2.3.2-cp313-cp313-wasix_wasm32.whl",
        sha256: "f2abcba47de3063e00fd960b17058bf14954fb3485e58153ba6925447d28af55",
    },
    // pandas (нативное wasix-колесо) + его чистые зависимости.
    Wheel {
        dir: "pandas",
        url: "https://pythonindex.wasix.org/packages/pandas-2.3.2-cp313-cp313-wasix_wasm32.whl",
        sha256: "9b7d0e64cd3bebe36dedb4a2d888a0df6dbc50a53011c2d6e96d4dac95eadd67",
    },
    Wheel {
        dir: "dateutil",
        url: "https://files.pythonhosted.org/packages/ec/57/56b9bcc3c9c6a792fcbaf139543cee77261f3651ca9da0c93f5c1221264b/python_dateutil-2.9.0.post0-py2.py3-none-any.whl",
        sha256: "a8b2bc7bffae282281c8140a97d3aa9c14da0b136dfe83f850eea9a5f7470427",
    },
    Wheel {
        dir: "six.py",
        url: "https://files.pythonhosted.org/packages/b7/ce/149a00dd41f10bc29e5921b496af8b574d8413afcd5e30dfa0ed46c2cc5e/six-1.17.0-py2.py3-none-any.whl",
        sha256: "4721f391ed90541fddacab5acf947aa0d3dc7d27b2e1e8eda2be8970586c3274",
    },
    Wheel {
        dir: "pytz",
        url: "https://files.pythonhosted.org/packages/ec/dd/96da98f892250475bdf2328112d7468abdd4acc7b902b6af23f4ed958ea0/pytz-2026.2-py2.py3-none-any.whl",
        sha256: "04156e608bee23d3792fd45c94ae47fae1036688e75032eea2e3bf0323d1f126",
    },
    Wheel {
        dir: "tzdata",
        url: "https://files.pythonhosted.org/packages/e5/6d/b53b99a9f2766d095985947a5782f1702cabb129a34f7a802d7197af832f/tzdata-2026.3-py2.py3-none-any.whl",
        sha256: "dc096730c87af6cab1b171c9d532be840741ff5d459015e7f6947bd7d7e54931",
    },
    Wheel {
        dir: "requests",
        url: "https://files.pythonhosted.org/packages/a0/f4/c67b0b3f1b9245e8d266f0f112c500d50e5b4e83cb6f3b71b6528104182a/requests-2.34.2-py3-none-any.whl",
        sha256: "2a0d60c172f83ac6ab31e4554906c0f3b3588d37b5cb939b1c061f4907e278e0",
    },
    Wheel {
        dir: "urllib3",
        url: "https://files.pythonhosted.org/packages/7f/3e/5db95bcf282c52709639744ca2a8b149baccf648e39c8cc87553df9eae0c/urllib3-2.7.0-py3-none-any.whl",
        sha256: "9fb4c81ebbb1ce9531cce37674bbc6f1360472bc18ca9a553ede278ef7276897",
    },
    Wheel {
        dir: "certifi",
        url: "https://files.pythonhosted.org/packages/ef/2f/c5464532e965badff2f4c4c1a3a83f5697f0d7c407ed0cda44aaa99bb451/certifi-2026.6.17-py3-none-any.whl",
        sha256: "2227dcbaafe0d2f59279d1762ddddc37783ed4354594f194ffc31d20f41fc3db",
    },
    Wheel {
        dir: "idna",
        url: "https://files.pythonhosted.org/packages/1e/5e/d4e9f1a599fb8e573b7b87160658329fbf28d19eac2718f51fc3def3aa5a/idna-3.18-py3-none-any.whl",
        sha256: "7f952cbe720b688055e3f87de14f5c3e5fdaa8bc3928985c4077ca689de849a2",
    },
    Wheel {
        dir: "charset_normalizer",
        url: "https://files.pythonhosted.org/packages/98/2b/f97f1c193fb855c345d678f5077d6926034db0722df74c8f057020e05a25/charset_normalizer-3.4.9-py3-none-any.whl",
        sha256: "68e5f26a1ad57ded6d1cfb85331d1c1a195314756471d97758c48498bb4dcdf5",
    },
];

/// Параметры провизии.
#[derive(Debug, Clone, Default)]
pub struct SetupOptions {
    /// Перекачать/переустановить всё, даже если уже на месте.
    pub force: bool,
}

/// Провизия всей песочницы в каталог `dir` (`data/sandbox/`). `progress` — колбэк
/// строк для stdout. Идемпотентно: уже установленное пропускается (кроме `force`).
pub async fn setup(
    dir: &Path,
    opts: &SetupOptions,
    loc: &Locale,
    mut progress: impl FnMut(&str),
) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| {
        loc.tf(
            "sandbox.setup.mkdir",
            &[("path", &dir.display().to_string())],
        )
    })?;
    let client = http_client(USER_AGENT, loc)?;

    let wasmer = ensure_wasmer(&client, dir, opts, loc, &mut progress).await?;
    ensure_python_webc(dir, &wasmer, opts, loc, &mut progress).await?;
    ensure_wheels(&client, dir, opts, loc, &mut progress).await?;
    warmup(dir, loc, &mut progress).await;

    progress(loc.t("sandbox.setup.done"));
    Ok(())
}

/// Прогрев кэша компиляции: один прогон компилирует `python.wasm` (+ нативные `.so`
/// numpy/pandas) в `<dir>/cache`, чтобы **первый реальный вызов** инструмента был
/// тёплым — без многосекундной компиляции на глазах у пользователя (заменяет «баннер
/// первого запуска»). «Лучшее усилие»: сбой прогрева не проваливает установку. Идёт
/// через реальный [`WasmerSandbox`], так что кэш и пути совпадают с рантаймом.
async fn warmup(dir: &Path, loc: &Locale, progress: &mut impl FnMut(&str)) {
    progress(loc.t("sandbox.setup.warmup.start"));
    let sb = WasmerSandbox::new(Some(dir.to_path_buf()));
    // `import pandas` тянет и интерпретатор, и нативные модули numpy/pandas (самый
    // тяжёлый путь компиляции); даже при сбое импорта интерпретатор уже в кэше.
    match sb
        .run("import pandas", false, Duration::from_secs(300), loc)
        .await
    {
        Ok(out) if out.exit_code == Some(0) => progress(loc.t("sandbox.setup.warmup.ok")),
        Ok(_) => progress(loc.t("sandbox.setup.warmup.partial")),
        Err(e) => progress(&loc.tf("sandbox.setup.warmup.skipped", &[("err", &e.to_string())])),
    }
}

/// Архив `wasmer` для платформы `(os, arch)` (чистая, тестируемая).
fn archive_for(os: &str, arch: &str) -> Option<&'static PlatformArchive> {
    ARCHIVES.iter().find(|a| a.os == os && a.arch == arch)
}

/// Гарантирует наличие бинаря `wasmer` в `dir`; возвращает путь к нему.
async fn ensure_wasmer(
    client: &reqwest::Client,
    dir: &Path,
    opts: &SetupOptions,
    loc: &Locale,
    progress: &mut impl FnMut(&str),
) -> Result<PathBuf> {
    if !opts.force
        && let Some(bin) = locate_wasmer(dir)
    {
        progress(&loc.tf(
            "sandbox.setup.wasmer.present",
            &[("path", &bin.display().to_string())],
        ));
        return Ok(bin);
    }
    let arch = archive_for(std::env::consts::OS, std::env::consts::ARCH).ok_or_else(|| {
        anyhow::anyhow!(
            "{}",
            loc.tf(
                "sandbox.setup.wasmer.no_platform",
                &[
                    ("os", std::env::consts::OS),
                    ("arch", std::env::consts::ARCH),
                ],
            )
        )
    })?;

    let archive_path = dir.join("wasmer.tar.gz");
    progress(&loc.tf(
        "sandbox.setup.wasmer.downloading",
        &[
            ("version", WASMER_VERSION),
            ("os", arch.os),
            ("arch", arch.arch),
        ],
    ));
    download_to_file(client, arch.url, &archive_path, arch.sha256, loc, progress).await?;

    let dist = dir.join("wasmer-dist");
    progress(loc.t("sandbox.setup.wasmer.extracting"));
    // Чистая переустановка каталога распаковки (устойчиво к прерванной прошлой).
    let _ = std::fs::remove_dir_all(&dist);
    extract_targz(&archive_path, &dist, loc)
        .with_context(|| loc.t("sandbox.setup.wasmer.extract_ctx").to_string())?;
    let _ = std::fs::remove_file(&archive_path);

    locate_wasmer(dir).ok_or_else(|| {
        anyhow::anyhow!(
            "{}",
            loc.tf(
                "sandbox.setup.wasmer.not_found",
                &[("path", &dist.display().to_string())],
            )
        )
    })
}

/// Гарантирует наличие `python.webc` (скачивается самим `wasmer` из реестра).
async fn ensure_python_webc(
    dir: &Path,
    wasmer: &Path,
    opts: &SetupOptions,
    loc: &Locale,
    progress: &mut impl FnMut(&str),
) -> Result<()> {
    let webc = dir.join("python.webc");
    if webc.is_file() && !opts.force {
        progress(loc.t("sandbox.setup.webc.present"));
        return Ok(());
    }
    progress(&loc.tf("sandbox.setup.webc.downloading", &[("pkg", PYTHON_PACKAGE)]));
    // Дом/кэш wasmer — под каталогом песочницы (самодостаточно, не в ~/.wasmer).
    let home = dir.join("wasmer-home");
    std::fs::create_dir_all(&home).ok();
    let out = tokio::process::Command::new(wasmer)
        .arg("package")
        .arg("download")
        .arg(PYTHON_PACKAGE)
        .arg("-o")
        .arg(&webc)
        .arg("--wasmer-dir")
        .arg(&home)
        .output()
        .await
        .with_context(|| {
            loc.tf(
                "sandbox.setup.webc.run",
                &[("path", &wasmer.display().to_string())],
            )
        })?;
    if !out.status.success() {
        bail!(
            "{}",
            loc.tf(
                "sandbox.setup.webc.failed",
                &[("stderr", String::from_utf8_lossy(&out.stderr).trim())],
            )
        );
    }
    anyhow::ensure!(
        webc.is_file(),
        "{}",
        loc.t("sandbox.setup.webc.not_created")
    );
    Ok(())
}

/// Гарантирует распаковку всех колёс в `site-packages/`.
async fn ensure_wheels(
    client: &reqwest::Client,
    dir: &Path,
    opts: &SetupOptions,
    loc: &Locale,
    progress: &mut impl FnMut(&str),
) -> Result<()> {
    let site = dir.join("site-packages");
    std::fs::create_dir_all(&site)
        .with_context(|| loc.t("sandbox.setup.wheels.mksite").to_string())?;
    for w in WHEELS {
        if site.join(w.dir).exists() && !opts.force {
            progress(&loc.tf("sandbox.setup.wheels.present", &[("name", w.dir)]));
            continue;
        }
        progress(&loc.tf("sandbox.setup.wheels.downloading", &[("name", w.dir)]));
        let bytes = download_bytes(client, w.url, w.sha256, loc).await?;
        progress(&loc.tf("sandbox.setup.wheels.extracting", &[("name", w.dir)]));
        unpack_zip(&bytes, &site, loc)
            .with_context(|| loc.tf("sandbox.setup.wheels.unpack", &[("name", w.dir)]))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_selection_by_platform() {
        assert!(archive_for("windows", "x86_64").is_some());
        assert!(archive_for("linux", "x86_64").is_some());
        assert!(archive_for("linux", "aarch64").is_some());
        assert!(archive_for("macos", "aarch64").is_some());
        // Неизвестная платформа — нет автоскачивания.
        assert!(archive_for("plan9", "x86_64").is_none());
        assert!(archive_for("windows", "riscv64").is_none());
    }

    #[test]
    fn lockfile_wheels_cover_numpy_and_requests_stack() {
        let dirs: Vec<&str> = WHEELS.iter().map(|w| w.dir).collect();
        for expected in [
            "numpy", "pandas", "dateutil", "pytz", "requests", "urllib3", "certifi", "idna",
        ] {
            assert!(dirs.contains(&expected), "нет колеса {expected}");
        }
        // Все URL — https, все sha256 — 64 hex-символа.
        for w in WHEELS {
            assert!(w.url.starts_with("https://"), "{}", w.url);
            assert_eq!(w.sha256.len(), 64, "{}", w.dir);
        }
    }
}
