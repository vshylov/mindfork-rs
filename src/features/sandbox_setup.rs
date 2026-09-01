//! Provisioning of Python-sandbox assets (`mindfork sandbox setup`, Phase 2 —
//! [docs/research/python-wasmer-sandbox.md](../../docs/research/python-wasmer-sandbox.md)).
//! Downloads into `data/sandbox/`: the `wasmer` binary (a platform tar.gz from GitHub),
//! `python.webc` (via `wasmer` itself), package wheels (numpy/pandas from the wasix index,
//! the requests stack and beautifulsoup4 from PyPI) — all via a **lock list with exact URL + sha256** (resilient
//! to "latest"). Provisioning is idempotent; the network layer is thin, the logic is
//! pure/testable.
//!
//! `features` layer: no TUI (the CLI command prints progress to stdout). The binary's
//! layout after unpacking matches what `shared::sandbox` looks for (the shared
//! resolver [`crate::shared::sandbox::locate_wasmer`]).

use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::shared::i18n::Locale;
use crate::shared::sandbox::{SandboxRunner, WasmerSandbox, locate_wasmer};

/// The `wasmer` version the lock list is pinned to (GitHub release tag `v<...>`).
pub const WASMER_VERSION: &str = "7.2.0";
/// The CPython package in the Wasmer registry (downloaded into `python.webc`).
///
/// **The `=` is load-bearing.** A wasmer package selector is a semver *range*,
/// not a pin: `python/python@3.13.5` resolves to the newest `3.13.x`, which is
/// how an unversioned `python/python` and a naively "pinned" one end up at the
/// same place. Measured against the live registry, 2026-08-30:
///
/// | Selector | Resolves to |
/// |---|---|
/// | `python/python` | 3.13.17 |
/// | `python/python@3.13.5` | **3.13.17** |
/// | `python/python@=3.13.5` | 3.13.5 |
///
/// What made that matter: the registry published 3.13.15/16/17 on 18–21 August
/// 2026, and their payload is 155,874,883 bytes against 3.13.5's 44,680,028 —
/// a build [`WASMER_VERSION`] cannot compile. Every fresh `sandbox setup` since
/// then produced a sandbox that could not run Python at all, failing with
/// `Validate("Failed to create V8 module: null module reference returned from
/// V8")` (and, earlier and more quietly, a partial warm-up). The runtime beside
/// this line was already pinned exactly; the package it runs was not.
///
/// 3.13.5 is the last version before that batch and is what every working
/// sandbox in this project holds — verified byte for byte, sha256
/// `c03ebe0946e66edf598fd7a1f192101f60e4e9c0095aecd04e049989692bdcab`.
/// Bumping it means downloading the candidate and running
/// `runs_real_python_in_sandbox` against a **freshly provisioned** directory:
/// an existing sandbox keeps working across a bad publish, so only a fresh one
/// can tell you.
const PYTHON_PACKAGE: &str = "python/python@=3.13.5";
/// sha256 of the `python.webc` that [`PYTHON_PACKAGE`] resolves to — the digest
/// the comment above has been quoting since the August 2026 incident.
///
/// It is checked on the **finished file**, not on a download, because this one
/// asset is fetched by the `wasmer` binary rather than by our client: there is
/// no URL for the lock list to pin. Until this constant existed the only
/// post-condition was that the file had been created, which was the single gap
/// in the promise SECURITY.md makes about this command
/// ([docs/research/code-signing.md](../../docs/research/code-signing.md) §3.2).
const PYTHON_PACKAGE_SHA256: &str =
    "c03ebe0946e66edf598fd7a1f192101f60e4e9c0095aecd04e049989692bdcab";
/// User-Agent for downloads (GitHub/PyPI sometimes reject an empty UA).
const USER_AGENT: &str = "mindfork-sandbox-setup";

/// A platform `wasmer` archive from GitHub (tar.gz). `os`/`arch` — from
/// [`std::env::consts`]. Unpacked whole into `<dir>/wasmer-dist/`.
struct PlatformArchive {
    os: &'static str,
    arch: &'static str,
    url: &'static str,
    sha256: &'static str,
}

/// Lock list of `wasmer` v7.2.0 archives (sha256 — from GitHub release assets `digest`).
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

/// A Python-package wheel (`.whl` = zip). `dir` — the package's directory name in `site-packages`
/// (for idempotency: a directory exists → the wheel is already unpacked).
struct Wheel {
    dir: &'static str,
    url: &'static str,
    sha256: &'static str,
}

/// Lock list of wheels. **numpy**/**pandas** — native wasix wheels from
/// `pythonindex.wasix.org`; their pure dependencies (dateutil/six/pytz/tzdata), the
/// **requests stack** (requests/urllib3/certifi/idna/charset_normalizer) and
/// **beautifulsoup4** (+ soupsieve/typing_extensions) — pure Python from PyPI
/// (`py3-none-any`). Versions are pinned; sha256 — from the index/PyPI.
const WHEELS: &[Wheel] = &[
    Wheel {
        dir: "numpy",
        url: "https://pythonindex.wasix.org/packages/numpy-2.3.2-cp313-cp313-wasix_wasm32.whl",
        sha256: "f2abcba47de3063e00fd960b17058bf14954fb3485e58153ba6925447d28af55",
    },
    // pandas (a native wasix wheel) + its pure dependencies.
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
    // beautifulsoup4 (HTML parsing, the natural companion to requests) + its dependencies.
    // The package directory is `bs4`, not the distribution name.
    Wheel {
        dir: "bs4",
        url: "https://files.pythonhosted.org/packages/88/c6/92fcd42f1ba33e1184263f25bfabf3d27c383410470f169e4b8163bf9c17/beautifulsoup4-4.15.0-py3-none-any.whl",
        sha256: "d6f88de62e1d4e38ecb1077eb9724cd0eff29d2a08ca16a401e9b9e93f117cf9",
    },
    Wheel {
        dir: "soupsieve",
        url: "https://files.pythonhosted.org/packages/0f/2c/437fe806897c2d6cfdc3ee43a18da8bf8e568530a4ae9bac781541ca9896/soupsieve-2.9.1-py3-none-any.whl",
        sha256: "4f4477399246b7a0c720a88ca2454b11cd6bb9ae4c9d170140786e916776c14c",
    },
    Wheel {
        dir: "typing_extensions.py",
        url: "https://files.pythonhosted.org/packages/49/d3/b8441a820a491ddfc024b0b0cf0393375b75ea13866d9c66727e54c2fc80/typing_extensions-4.16.0-py3-none-any.whl",
        sha256: "481caa481374e813c1b176ada14e97f1f67a4539ce9cfeb3f350d78d6370c2e8",
    },
];

/// Provisioning options.
#[derive(Debug, Clone, Default)]
pub struct SetupOptions {
    /// Re-download/reinstall everything, even if already present.
    pub force: bool,
}

/// Provisions the whole sandbox into the `dir` directory (`data/sandbox/`). `progress` — a
/// string callback for stdout. Idempotent: already-installed assets are skipped (unless `force`).
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
    let client = http_client(loc)?;

    let wasmer = ensure_wasmer(&client, dir, opts, loc, &mut progress).await?;
    ensure_python_webc(dir, &wasmer, opts, loc, &mut progress).await?;
    ensure_wheels(&client, dir, opts, loc, &mut progress).await?;
    warmup(dir, loc, &mut progress).await;

    progress(loc.t("sandbox.setup.done"));
    Ok(())
}

/// Warms the compilation cache: one run compiles `python.wasm` (+ the native `.so`
/// for numpy/pandas) into `<dir>/cache`, so the **first real tool call** is
/// warm — no multi-second compilation in front of the user (replaces the "first-run
/// banner"). "Best effort": a warmup failure doesn't fail the install. Runs
/// through a real [`WasmerSandbox`], so the cache and paths match the runtime.
async fn warmup(dir: &Path, loc: &Locale, progress: &mut impl FnMut(&str)) {
    progress(loc.t("sandbox.setup.warmup.start"));
    let sb = WasmerSandbox::new(Some(dir.to_path_buf()));
    // `import pandas` pulls in both the interpreter and the native numpy/pandas modules (the
    // heaviest compilation path); even if the import fails, the interpreter is already cached.
    match sb
        .run("import pandas", false, Duration::from_secs(300), loc)
        .await
    {
        Ok(out) if out.exit_code == Some(0) => progress(loc.t("sandbox.setup.warmup.ok")),
        Ok(_) => progress(loc.t("sandbox.setup.warmup.partial")),
        Err(e) => progress(&loc.tf("sandbox.setup.warmup.skipped", &[("err", &e.to_string())])),
    }
}

/// The `wasmer` archive for platform `(os, arch)` (pure, testable).
fn archive_for(os: &str, arch: &str) -> Option<&'static PlatformArchive> {
    ARCHIVES.iter().find(|a| a.os == os && a.arch == arch)
}

/// Ensures the `wasmer` binary is present in `dir`; returns its path.
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
    // A clean reinstall of the unpack directory (resilient to a previous interrupted one).
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

/// Ensures `python.webc` is present **and is the pinned build** (downloaded by
/// `wasmer` itself from the registry, verified here against
/// [`PYTHON_PACKAGE_SHA256`]).
///
/// A file that is present but does not match is replaced rather than refused:
/// this command is provisioning, and a stale or truncated asset is exactly what
/// it exists to fix. Only a *freshly downloaded* file that still mismatches is
/// fatal — at that point the registry is serving something other than what we
/// pinned, and continuing would defeat the check.
async fn ensure_python_webc(
    dir: &Path,
    wasmer: &Path,
    opts: &SetupOptions,
    loc: &Locale,
    progress: &mut impl FnMut(&str),
) -> Result<()> {
    let webc = dir.join("python.webc");
    if webc.is_file() && !opts.force {
        match verify_file_sha256(&webc, PYTHON_PACKAGE_SHA256, PYTHON_PACKAGE, loc).await {
            Ok(()) => {
                progress(loc.t("sandbox.setup.webc.present"));
                return Ok(());
            }
            Err(err) => {
                progress(&loc.tf("sandbox.setup.webc.stale", &[("reason", &err.to_string())]))
            }
        }
    }
    progress(&loc.tf("sandbox.setup.webc.downloading", &[("pkg", PYTHON_PACKAGE)]));
    // wasmer's home/cache — under the sandbox directory (self-contained, not in ~/.wasmer).
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
    verify_file_sha256(&webc, PYTHON_PACKAGE_SHA256, PYTHON_PACKAGE, loc).await
}

/// Ensures all wheels are unpacked into `site-packages/`.
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
        unpack_wheel(&bytes, &site, loc)
            .with_context(|| loc.tf("sandbox.setup.wheels.unpack", &[("name", w.dir)]))?;
    }
    Ok(())
}

/// An HTTP client with a User-Agent (rustls, as elsewhere in the project).
fn http_client(loc: &Locale) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .with_context(|| loc.t("sandbox.setup.http_client").to_string())
}

/// Streams `url` into the file `dest` (large archives aren't buffered in memory),
/// hashing sha256 on the fly; verifies it against `expected`.
async fn download_to_file(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    expected: &str,
    loc: &Locale,
    progress: &mut impl FnMut(&str),
) -> Result<()> {
    let resp = client
        .get(url)
        .send()
        .await
        .with_context(|| loc.tf("sandbox.setup.request", &[("url", url)]))?
        .error_for_status()
        .with_context(|| loc.tf("sandbox.setup.download", &[("url", url)]))?;
    let total = resp.content_length();
    let mut file = tokio::fs::File::create(dest).await.with_context(|| {
        loc.tf(
            "sandbox.setup.create_file",
            &[("path", &dest.display().to_string())],
        )
    })?;
    let mut hasher = Sha256::new();
    let mut stream = resp.bytes_stream();
    let mut done: u64 = 0;
    let mut next_report: u64 = 32 * 1024 * 1024; // report every ~32 MB
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.with_context(|| loc.t("sandbox.setup.read_stream").to_string())?;
        hasher.update(&chunk);
        file.write_all(&chunk)
            .await
            .with_context(|| loc.t("sandbox.setup.write_file").to_string())?;
        done += chunk.len() as u64;
        if done >= next_report {
            match total {
                Some(t) => progress(&loc.tf(
                    "sandbox.setup.progress_bytes",
                    &[
                        ("done", &(done >> 20).to_string()),
                        ("total", &(t >> 20).to_string()),
                    ],
                )),
                None => progress(&loc.tf(
                    "sandbox.setup.progress_bytes_unknown",
                    &[("done", &(done >> 20).to_string())],
                )),
            }
            next_report += 32 * 1024 * 1024;
        }
    }
    file.flush()
        .await
        .with_context(|| loc.t("sandbox.setup.flush").to_string())?;
    verify_sha256(&hasher.finalize(), expected, url, loc)
}

/// Downloads `url` entirely into memory (small wheels), verifies sha256.
async fn download_bytes(
    client: &reqwest::Client,
    url: &str,
    expected: &str,
    loc: &Locale,
) -> Result<Vec<u8>> {
    let bytes = client
        .get(url)
        .send()
        .await
        .with_context(|| loc.tf("sandbox.setup.request", &[("url", url)]))?
        .error_for_status()
        .with_context(|| loc.tf("sandbox.setup.download", &[("url", url)]))?
        .bytes()
        .await
        .with_context(|| loc.t("sandbox.setup.read_body").to_string())?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    verify_sha256(&hasher.finalize(), expected, url, loc)?;
    Ok(bytes.to_vec())
}

/// Verifies a file already on disk against an expected sha256, streaming it in
/// chunks — `python.webc` is ~45 MB, and the two callers
/// ([`ensure_python_webc`]) have no bytes in hand to hash on the way past.
/// `label` names the subject in an error, the way a URL does for a download.
async fn verify_file_sha256(path: &Path, expected: &str, label: &str, loc: &Locale) -> Result<()> {
    use tokio::io::AsyncReadExt;

    let mut file = tokio::fs::File::open(path).await.with_context(|| {
        loc.tf(
            "sandbox.setup.open_file",
            &[("path", &path.display().to_string())],
        )
    })?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buf)
            .await
            .with_context(|| loc.t("sandbox.setup.read_file").to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    verify_sha256(&hasher.finalize(), expected, label, loc)
}

/// Verifies the hash (raw digest bytes) against the expected hex; the error names the URL.
fn verify_sha256(digest: &[u8], expected: &str, url: &str, loc: &Locale) -> Result<()> {
    let got = hex_lower(digest);
    if got.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        bail!(
            "{}",
            loc.tf(
                "sandbox.setup.sha_mismatch",
                &[("url", url), ("expected", expected), ("got", &got)],
            )
        )
    }
}

/// Bytes → a hex string (lowercase), without the `hex` crate.
fn hex_lower(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Unpacks a tar.gz archive into the `dest` directory.
fn extract_targz(archive: &Path, dest: &Path, loc: &Locale) -> Result<()> {
    let file = std::fs::File::open(archive).with_context(|| {
        loc.tf(
            "sandbox.setup.open_archive",
            &[("path", &archive.display().to_string())],
        )
    })?;
    let gz = flate2::read::GzDecoder::new(std::io::BufReader::new(file));
    let mut ar = tar::Archive::new(gz);
    std::fs::create_dir_all(dest)?;
    ar.unpack(dest).with_context(|| {
        loc.tf(
            "sandbox.setup.extract_to",
            &[("path", &dest.display().to_string())],
        )
    })?;
    Ok(())
}

/// Unpacks a wheel (zip) into the `site` directory (zip-slip protection via
/// `enclosed_name`, as in `features::backup`).
fn unpack_wheel(bytes: &[u8], site: &Path, loc: &Locale) -> Result<()> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))
        .with_context(|| loc.t("sandbox.setup.open_wheel").to_string())?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let Some(rel) = entry.enclosed_name() else {
            bail!(
                "{}",
                loc.tf("sandbox.setup.unsafe_wheel", &[("name", entry.name())])
            );
        };
        let out = site.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut buf = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut buf)?;
        std::fs::write(&out, &buf)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {

    /// The `=` in [`PYTHON_PACKAGE`] is not decoration: without it the selector
    /// is a semver range and the registry hands back the newest `3.13.x`, which
    /// is the exact shape that broke every fresh sandbox install in August 2026.
    /// This is here so that removing it fails a test instead of a user.
    #[test]
    fn the_python_package_is_pinned_exactly() {
        let (_name, version) = PYTHON_PACKAGE
            .split_once('@')
            .expect("the package must carry a version selector");
        assert!(
            version.starts_with('='),
            "`{PYTHON_PACKAGE}` is a semver *range*, not a pin - it resolves to the \
             newest matching version. Use `@=<version>`."
        );
    }

    use super::*;
    use crate::shared::i18n::{Lang, locale};

    /// The reference locale for tests (error texts aren't checked here — ru byte-for-byte).
    fn ru() -> &'static Locale {
        locale(Lang::Ru)
    }

    #[test]
    fn archive_selection_by_platform() {
        assert!(archive_for("windows", "x86_64").is_some());
        assert!(archive_for("linux", "x86_64").is_some());
        assert!(archive_for("linux", "aarch64").is_some());
        assert!(archive_for("macos", "aarch64").is_some());
        // An unknown platform — no auto-download.
        assert!(archive_for("plan9", "x86_64").is_none());
        assert!(archive_for("windows", "riscv64").is_none());
    }

    #[test]
    fn hex_encoding_is_lowercase() {
        assert_eq!(hex_lower(&[0x00, 0x0f, 0xff, 0xa5]), "000fffa5");
    }

    #[test]
    fn verify_sha256_matches_case_insensitively() {
        let digest = Sha256::digest(b"hello");
        let hex = hex_lower(&digest);
        assert!(verify_sha256(&digest, &hex, "u", ru()).is_ok());
        assert!(verify_sha256(&digest, &hex.to_uppercase(), "u", ru()).is_ok());
        assert!(verify_sha256(&digest, "deadbeef", "u", ru()).is_err());
    }

    #[test]
    fn sha_mismatch_error_is_localized() {
        // A regression against a forgotten `loc`: an en message with no Cyrillic, ru — Russian.
        let digest = Sha256::digest(b"hello");
        let en = verify_sha256(&digest, "deadbeef", "u", locale(Lang::En))
            .unwrap_err()
            .to_string();
        assert!(en.contains("sha256 mismatch"), "{en}");
        assert!(!en.chars().any(|c| ('а'..='я').contains(&c)), "{en}");
        let r = verify_sha256(&digest, "deadbeef", "u", ru())
            .unwrap_err()
            .to_string();
        assert!(r.contains("не совпал"), "{r}");
    }

    #[test]
    fn lockfile_wheels_cover_numpy_and_requests_stack() {
        let dirs: Vec<&str> = WHEELS.iter().map(|w| w.dir).collect();
        for expected in [
            "numpy",
            "pandas",
            "dateutil",
            "pytz",
            "requests",
            "urllib3",
            "certifi",
            "idna",
            // beautifulsoup4 lands as `bs4`; soupsieve/typing_extensions are its dependencies.
            "bs4",
            "soupsieve",
            "typing_extensions.py",
        ] {
            assert!(dirs.contains(&expected), "missing wheel {expected}");
        }
        // All URLs — https, all sha256 — 64 hex characters.
        for w in WHEELS {
            assert!(w.url.starts_with("https://"), "{}", w.url);
            assert_eq!(w.sha256.len(), 64, "{}", w.dir);
        }
    }

    #[test]
    fn unpack_wheel_extracts_into_site_packages() {
        // A crafted zip as a "wheel": a package file + dist-info.
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(Cursor::new(&mut buf));
            let opts = zip::write::SimpleFileOptions::default();
            use std::io::Write as _;
            w.start_file("mypkg/__init__.py", opts).unwrap();
            w.write_all(b"x = 1\n").unwrap();
            w.start_file("mypkg-1.0.dist-info/METADATA", opts).unwrap();
            w.write_all(b"Name: mypkg\n").unwrap();
            w.finish().unwrap();
        }
        let dir = tempfile::tempdir().unwrap();
        unpack_wheel(&buf, dir.path(), ru()).unwrap();
        assert!(dir.path().join("mypkg/__init__.py").is_file());
        assert!(dir.path().join("mypkg-1.0.dist-info/METADATA").is_file());
    }

    #[test]
    fn extract_targz_roundtrip() {
        // Build a small tar.gz with a bin/wasmer file and unpack it.
        let mut gz = Vec::new();
        {
            let enc = flate2::write::GzEncoder::new(&mut gz, flate2::Compression::default());
            let mut tb = tar::Builder::new(enc);
            let data = b"#!fake wasmer\n";
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            tb.append_data(&mut header, "bin/wasmer", &data[..])
                .unwrap();
            tb.into_inner().unwrap().finish().unwrap();
        }
        let archive = tempfile::tempdir().unwrap();
        let apath = archive.path().join("a.tar.gz");
        std::fs::write(&apath, &gz).unwrap();
        let dest = tempfile::tempdir().unwrap();
        extract_targz(&apath, dest.path(), ru()).unwrap();
        assert!(dest.path().join("bin/wasmer").is_file());
    }

    /// The one asset no lock-list row can cover reads its digest from a hand-copied
    /// constant, so the shape of that constant is worth a test: a truncated or
    /// upper-cased paste would only be discovered by a user whose `sandbox setup`
    /// suddenly refuses a perfectly good download.
    #[test]
    fn the_python_package_digest_is_a_well_formed_sha256() {
        assert_eq!(PYTHON_PACKAGE_SHA256.len(), 64);
        assert!(
            PYTHON_PACKAGE_SHA256
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
            "expected 64 lowercase hex characters, got `{PYTHON_PACKAGE_SHA256}`"
        );
    }

    #[tokio::test]
    async fn a_file_is_verified_against_its_own_digest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("payload.bin");
        // Larger than the 64 KB read buffer, so the streaming loop runs more than once.
        let body: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&path, &body).unwrap();
        let digest = hex_lower(&Sha256::digest(&body));

        verify_file_sha256(&path, &digest, "payload", ru())
            .await
            .expect("the digest of the bytes just written must match");

        let err = verify_file_sha256(&path, &"0".repeat(64), "payload", ru())
            .await
            .expect_err("a wrong digest must be refused");
        assert!(err.to_string().contains("payload"), "got: {err}");
    }

    /// The digest above is only useful if the registry still serves the build it
    /// names — a pin that has drifted turns every fresh `sandbox setup` into a
    /// refusal, which is a worse failure than the one it guards against. So this
    /// downloads `python.webc` for real, into an empty directory, and verifies it.
    ///
    /// Needs a `wasmer` binary, which it borrows from a sandbox this machine has
    /// already provisioned (`MINDFORK_SANDBOX_DIR`, else `target/debug/data/sandbox`);
    /// it never touches that directory's own `python.webc`. Run it when bumping
    /// [`PYTHON_PACKAGE`], together with `runs_real_python_in_sandbox`.
    #[tokio::test]
    #[ignore = "downloads ~45 MB from the Wasmer registry"]
    async fn live_the_registry_still_serves_the_pinned_python_build() {
        let host = std::env::var("MINDFORK_SANDBOX_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("target/debug/data/sandbox"));
        let Some(wasmer) = locate_wasmer(&host) else {
            eprintln!("skip: no wasmer under {}", host.display());
            return;
        };
        let fresh = tempfile::tempdir().unwrap();
        ensure_python_webc(
            fresh.path(),
            &wasmer,
            &SetupOptions { force: false },
            ru(),
            &mut |m: &str| eprintln!("{m}"),
        )
        .await
        .expect("a fresh download must match PYTHON_PACKAGE_SHA256");
    }

    #[tokio::test]
    async fn verifying_a_missing_file_names_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("absent.bin");
        let err = verify_file_sha256(&path, &"0".repeat(64), "absent", ru())
            .await
            .expect_err("a missing file cannot verify");
        assert!(err.to_string().contains("absent.bin"), "got: {err:#}");
    }
}
