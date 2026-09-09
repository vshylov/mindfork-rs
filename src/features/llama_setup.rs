//! Downloading llama.cpp `llama-server` builds (`mindfork llama backends |
//! setup | installed`) — stage 1 of
//! [docs/research/llama-cpp-download.md](../../docs/research/llama-cpp-download.md).
//!
//! Where `features::sandbox_setup` provisions from a **pinned table** (wasmer:
//! four rows, four URLs, four digests), this one **derives**: llama.cpp
//! publishes ~13 nightly builds a day across 27 assets whose names have been
//! renamed twice in fourteen months (research §3.3), so the list of backends is
//! read out of the release itself and nothing about it is enumerated here.
//!
//! The whole flow: resolve a release (§3.1 — `releases/latest` is *not* a
//! build, every `bNNNNN` tag is a prerelease), derive the backends this
//! platform can run (§3.3's anchored parse), download the chosen one plus, on
//! Windows, its CUDA runtime (§3.4 — without it the CUDA backend silently does
//! not load), verify every byte against the release's own sha256, unpack into
//! `data/llama/<backend>-<tag>/` and prove the result runs.
//!
//! `features` layer: no TUI, no stdout — progress is a `impl FnMut(&str)`
//! callback the CLI supplies (the `sandbox_setup::setup` shape).

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::shared::i18n::Locale;

/// The releases endpoint of the upstream repository.
const RELEASES_API: &str = "https://api.github.com/repos/ggml-org/llama.cpp/releases";
/// User-Agent for the API and the downloads (GitHub rejects an empty one).
const USER_AGENT: &str = "mindfork-llama-setup";
/// How many of the newest releases to look through when no `--build` is given.
///
/// Not one: the newest release can be a semver tag (`v0.4.0`) whose only asset
/// is a 7-byte `nightly-tag.txt`, and then page 1 yields no backends at all
/// (research §3.1). Five turns that into a retry instead of a failure.
const RELEASE_SCAN: usize = 5;
/// Connect timeout for every request. The sandbox's client sets **no timeout of
/// any kind** (`sandbox_setup::http_client`), which is a hung command waiting to
/// happen on an asset this size; `shared::api::http` picked 10 s for the same
/// reason and this follows it.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Read timeout — the gap between two chunks, not the total transfer. A 645 MB
/// download over a slow line is not an error; a socket that has gone quiet is.
const READ_TIMEOUT: Duration = Duration::from_secs(60);
/// How often the byte counter is reported (the sandbox's cadence).
const PROGRESS_STEP: u64 = 32 * 1024 * 1024;
/// How long the freshly installed binary gets to answer `--version` /
/// `--list-devices`. Both are model-free and return in milliseconds; the limit
/// only bounds a binary that cannot load its libraries at all.
const PROBE_TIMEOUT: Duration = Duration::from_secs(60);

// -------- The release, as the API reports it --------

/// One asset of a GitHub release.
#[derive(Debug, Clone, Deserialize)]
pub struct Asset {
    pub name: String,
    pub size: u64,
    /// `sha256:<hex>` — published by GitHub for every asset, including releases
    /// from 2025 (research §3.3). This is what makes R3 possible without a
    /// hand-maintained pin table.
    #[serde(default)]
    pub digest: Option<String>,
    pub browser_download_url: String,
}

/// A release: either a build tag `bNNNNN` or a semver tag with no binaries.
#[derive(Debug, Clone, Deserialize)]
pub struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub published_at: Option<String>,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

/// A backend offered for the running platform: the server archive and, for a
/// `cuda-*` backend on Windows, the CUDA runtime archive that must land in the
/// same directory.
#[derive(Debug, Clone)]
pub struct Backend {
    /// `cpu`, `cuda-12.4`, `vulkan`, `rocm-10.0`, … — derived, never enumerated.
    pub id: String,
    pub asset: Asset,
    pub cudart: Option<Asset>,
}

impl Backend {
    /// Total bytes to download for this backend.
    pub fn download_size(&self) -> u64 {
        self.asset.size + self.cudart.as_ref().map_or(0, |c| c.size)
    }

    /// Does this backend need a CUDA runtime archive it did not find?
    pub fn cudart_missing(&self) -> bool {
        self.needs_cudart() && self.cudart.is_none()
    }

    /// A `cuda-*` backend links `cublas`/`cudart`, which upstream ships in a
    /// separate archive (research §3.4).
    pub fn needs_cudart(&self) -> bool {
        self.id.starts_with("cuda-")
    }
}

/// What `mindfork llama backends` resolved.
#[derive(Debug, Clone)]
pub struct Listing {
    pub tag: String,
    /// `published_at` trimmed to a date, or empty when the API omitted it.
    pub date: String,
    pub backends: Vec<Backend>,
}

/// One directory under `data/llama/`.
#[derive(Debug, Clone)]
pub struct Install {
    pub backend: String,
    pub tag: String,
    pub dir: PathBuf,
    pub bytes: u64,
    /// Is the server binary actually there (an interrupted install is not).
    pub binary_ok: bool,
}

/// Options for [`setup`].
#[derive(Debug, Clone)]
pub struct SetupOptions {
    /// The backend id, as `backends` reports it.
    pub backend: String,
    /// A build tag to pin (`b10883`); `None` — the newest one with binaries.
    pub build: Option<String>,
    /// Re-download and reinstall even if the directory is already there.
    pub force: bool,
    /// Fetch the CUDA runtime alongside a `cuda-*` backend. `false` only when
    /// the user passed `--no-cudart` because the runtime is already on the host.
    pub cudart: bool,
}

/// What [`setup`] produced.
///
/// Only `binary` has a consumer in stage 1 (the CLI's closing hint); the rest is
/// what stage 2's `--set-binary` writes into the three managed configs and what
/// the live smoke asserts on, so it is deliberate ahead-of-consumer API rather
/// than a field nobody wanted (CLAUDE.md §Pitfalls).
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct Installed {
    pub backend: String,
    pub tag: String,
    pub dir: PathBuf,
    pub binary: PathBuf,
    /// The `--version` line the installed binary printed.
    pub version: String,
    /// Compute devices `--list-devices` reported (empty on a CPU build, and on
    /// a GPU build whose driver or runtime is missing).
    pub devices: Vec<String>,
}

// -------- The name derivation (pure; research §3.3) --------

/// Archive kind, read off the asset's name rather than assumed from the OS:
/// upstream shipped Linux and macOS builds as `.zip` until 2025 and as
/// `.tar.gz` since (research §3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ext {
    Zip,
    TarGz,
}

impl Ext {
    fn of(name: &str) -> Option<Self> {
        if let Some(rest) = name.strip_suffix(".tar.gz") {
            (!rest.is_empty()).then_some(Ext::TarGz)
        } else if let Some(rest) = name.strip_suffix(".zip") {
            (!rest.is_empty()).then_some(Ext::Zip)
        } else {
            None
        }
    }

    fn strip(self, name: &str) -> &str {
        let suffix = match self {
            Ext::Zip => ".zip",
            Ext::TarGz => ".tar.gz",
        };
        name.strip_suffix(suffix).unwrap_or(name)
    }
}

/// `std::env::consts::OS` → the token upstream puts in an asset name.
pub fn os_token(os: &str) -> Option<&'static str> {
    match os {
        "windows" => Some("win"),
        "linux" => Some("ubuntu"),
        "macos" => Some("macos"),
        _ => None,
    }
}

/// `std::env::consts::ARCH` → the token upstream puts in an asset name.
pub fn arch_token(arch: &str) -> Option<&'static str> {
    match arch {
        "x86_64" => Some("x64"),
        "aarch64" => Some("arm64"),
        _ => None,
    }
}

/// The backend id of a server archive, or `None` when the name is not one for
/// this platform.
///
/// The shape, anchored on **both** ends so that nothing has to be guessed:
///
/// ```text
/// llama-<tag>-bin-<os>[-<backend…>]-<arch>.<ext>
/// ```
///
/// `<backend…>` is everything in between, joined with `-`; **empty means
/// `cpu`**, which is how the Linux CPU build (`llama-b10883-bin-ubuntu-x64`)
/// spells itself. Anything that does not match exactly is skipped rather than
/// interpreted — that is what drops `ui`, `xcframework`, `android-arm64`,
/// `ubuntu-s390x`, `310p-openEuler-x86`, `910b-openEuler-x86-aclgraph` and
/// `macos-arm64-kleidiai` without a special case for any of them.
fn backend_of(name: &str, tag: &str, os_tok: &str, arch_tok: &str) -> Option<(String, Ext)> {
    let ext = Ext::of(name)?;
    let stem = ext.strip(name);
    let rest = stem.strip_prefix(&format!("llama-{tag}-bin-"))?;
    let mut parts = rest.split('-');
    if parts.next()? != os_tok {
        return None;
    }
    let middle: Vec<&str> = parts.collect();
    let (&last, head) = middle.split_last()?;
    if last != arch_tok {
        return None;
    }
    let id = if head.is_empty() {
        "cpu".to_string()
    } else {
        head.join("-")
    };
    (!id.is_empty()).then_some((id, ext))
}

/// The CUDA runtime archive paired with `backend` — by (backend token, arch),
/// and **without a build tag** in its name: `cudart-llama-bin-win-cuda-12.4-x64.zip`
/// sits in the same release as `llama-<tag>-bin-win-cuda-12.4-x64.zip`.
fn cudart_name(backend: &str, os_tok: &str, arch_tok: &str) -> String {
    format!("cudart-llama-bin-{os_tok}-{backend}-{arch_tok}.zip")
}

/// Every backend in `release` that this `(os, arch)` can run, sorted by id.
///
/// `os`/`arch` are [`std::env::consts`] values; an unsupported platform yields
/// an empty list rather than a guess.
pub fn backends(release: &Release, os: &str, arch: &str) -> Vec<Backend> {
    let (Some(os_tok), Some(arch_tok)) = (os_token(os), arch_token(arch)) else {
        return Vec::new();
    };
    let mut out: Vec<Backend> = Vec::new();
    for asset in &release.assets {
        let Some((id, _ext)) = backend_of(&asset.name, &release.tag_name, os_tok, arch_tok) else {
            continue;
        };
        if out.iter().any(|b| b.id == id) {
            continue; // first one wins; upstream has never published two
        }
        let cudart = id.starts_with("cuda-").then(|| {
            let want = cudart_name(&id, os_tok, arch_tok);
            release.assets.iter().find(|a| a.name == want).cloned()
        });
        out.push(Backend {
            id,
            asset: asset.clone(),
            cudart: cudart.flatten(),
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// The install directory's name: `<backend>-<tag>`, which reads back as what it
/// is and lets two builds coexist for a comparison.
pub fn install_name(backend: &str, tag: &str) -> String {
    format!("{backend}-{tag}")
}

/// Splits an install directory's name back into `(backend, tag)`. Backend ids
/// contain dashes (`cuda-12.4`) and tags do not, so the split is from the right.
pub fn split_install_name(name: &str) -> Option<(&str, &str)> {
    let (backend, tag) = name.rsplit_once('-')?;
    (!backend.is_empty() && !tag.is_empty()).then_some((backend, tag))
}

/// The server executable's name on this platform.
pub fn server_binary_name() -> &'static str {
    if cfg!(windows) {
        "llama-server.exe"
    } else {
        "llama-server"
    }
}

/// `sha256:<hex>` → `<hex>`. A digest in any other form is not understood, and
/// an asset we cannot verify is not installed (R3).
fn digest_hex(digest: &str) -> Option<&str> {
    let hex = digest.strip_prefix("sha256:")?;
    (hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit())).then_some(hex)
}

/// `b10883` → `10883`, so the tag can be compared with what the installed
/// binary reports.
pub fn tag_build_number(tag: &str) -> Option<u64> {
    tag.strip_prefix('b')?.parse::<u64>().ok()
}

/// The build number out of a `--version` line
/// (`version: 0.3.0-dev (build 10807, commit 163a4079…)`).
pub fn version_build_number(text: &str) -> Option<u64> {
    let at = text.find("build ")? + "build ".len();
    let digits: String = text[at..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse::<u64>().ok()
}

/// The devices `--list-devices` printed: every non-empty line after the header,
/// with the literal `(none)` meaning there are none.
pub fn parse_devices(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .skip(1)
        .map(str::trim)
        .filter(|l| !l.is_empty() && *l != "(none)")
        .map(str::to_string)
        .collect()
}

// -------- Resolving a release --------

/// The HTTP client: a User-Agent, real timeouts, and the user's `GITHUB_TOKEN`
/// when they have one (the unauthenticated API allows 60 requests an hour per
/// address — one per command invocation, so only a shared address can reach it).
fn http_client(loc: &Locale) -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT);
    if let Ok(token) = std::env::var("GITHUB_TOKEN")
        && !token.trim().is_empty()
    {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Ok(mut value) =
            reqwest::header::HeaderValue::from_str(&format!("Bearer {}", token.trim()))
        {
            value.set_sensitive(true);
            headers.insert(reqwest::header::AUTHORIZATION, value);
            builder = builder.default_headers(headers);
        }
    }
    builder
        .build()
        .with_context(|| loc.t("llamacpp.setup.http_client").to_string())
}

/// GETs `url` and returns the body, turning a rate-limit refusal into a message
/// that says what actually happened instead of a bare `403`.
async fn api_get(client: &reqwest::Client, url: &str, loc: &Locale) -> Result<String> {
    let resp = client
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .with_context(|| loc.tf("llamacpp.setup.request", &[("url", url)]))?;
    let status = resp.status();
    if status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::TOO_MANY_REQUESTS
    {
        let remaining = resp
            .headers()
            .get("x-ratelimit-remaining")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if remaining == "0" {
            bail!("{}", loc.t("llamacpp.setup.rate_limited"));
        }
    }
    resp.error_for_status()
        .with_context(|| loc.tf("llamacpp.setup.download", &[("url", url)]))?
        .text()
        .await
        .with_context(|| loc.t("llamacpp.setup.read_body").to_string())
}

/// Resolves the release to install from: the pinned `build`, or the newest of
/// the [`RELEASE_SCAN`] most recent that actually carries binaries for this
/// platform.
async fn fetch_release(
    client: &reqwest::Client,
    build: Option<&str>,
    os: &str,
    arch: &str,
    loc: &Locale,
) -> Result<Release> {
    if let Some(tag) = build {
        let url = format!("{RELEASES_API}/tags/{tag}");
        let body = api_get(client, &url, loc).await?;
        let release: Release = serde_json::from_str(&body)
            .with_context(|| loc.tf("llamacpp.setup.release_parse", &[("tag", tag)]))?;
        return Ok(release);
    }
    let url = format!("{RELEASES_API}?per_page={RELEASE_SCAN}");
    let body = api_get(client, &url, loc).await?;
    let releases: Vec<Release> = serde_json::from_str(&body)
        .with_context(|| loc.t("llamacpp.setup.releases_parse").to_string())?;
    releases
        .into_iter()
        .find(|r| !backends(r, os, arch).is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "{}",
                loc.tf(
                    "llamacpp.setup.no_release",
                    &[
                        ("count", &RELEASE_SCAN.to_string()),
                        ("os", os),
                        ("arch", arch)
                    ],
                )
            )
        })
}

/// Refuses a platform upstream publishes nothing for, by name rather than with
/// an empty list (the `sandbox.setup.wasmer.no_platform` shape).
fn check_platform(os: &str, arch: &str, loc: &Locale) -> Result<()> {
    if os_token(os).is_some() && arch_token(arch).is_some() {
        return Ok(());
    }
    bail!(
        "{}",
        loc.tf("llamacpp.setup.no_platform", &[("os", os), ("arch", arch)])
    )
}

/// `mindfork llama backends`: what this platform can run, out of the resolved
/// release.
pub async fn list_backends(build: Option<&str>, loc: &Locale) -> Result<Listing> {
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    check_platform(os, arch, loc)?;
    let client = http_client(loc)?;
    let release = fetch_release(&client, build, os, arch, loc).await?;
    let backends = backends(&release, os, arch);
    if backends.is_empty() {
        bail!(
            "{}",
            loc.tf(
                "llamacpp.setup.no_backends",
                &[("tag", &release.tag_name), ("os", os), ("arch", arch)],
            )
        );
    }
    Ok(Listing {
        date: release
            .published_at
            .as_deref()
            .and_then(|p| p.split('T').next())
            .unwrap_or("")
            .to_string(),
        tag: release.tag_name,
        backends,
    })
}

/// Renders a [`Listing`] as printable lines, marking what is already installed.
/// The layer boundary: this builds the text, the CLI prints it.
pub fn render_backends(listing: &Listing, root: &Path, loc: &Locale) -> Vec<String> {
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    let mut lines = vec![loc.tf(
        "llamacpp.backends.header",
        &[
            ("tag", &listing.tag),
            ("date", &listing.date),
            ("os", os),
            ("arch", arch),
        ],
    )];
    let width = listing
        .backends
        .iter()
        .map(|b| b.id.chars().count())
        .max()
        .unwrap_or(0);
    for b in &listing.backends {
        let mut notes: Vec<String> = Vec::new();
        if b.cudart.is_some() {
            notes.push(loc.t("llamacpp.backends.with_cudart").to_string());
        } else if b.needs_cudart() {
            notes.push(loc.t("llamacpp.backends.no_cudart").to_string());
        }
        if root.join(install_name(&b.id, &listing.tag)).is_dir() {
            notes.push(loc.t("llamacpp.backends.installed").to_string());
        }
        let note = if notes.is_empty() {
            String::new()
        } else {
            format!("  ({})", notes.join(", "))
        };
        lines.push(format!(
            "  {:width$}  {:>5} MB{}",
            b.id,
            mib(b.download_size()),
            note,
            width = width
        ));
    }
    lines
}

/// Bytes → whole mebibytes, the unit every size in this command is reported in.
fn mib(bytes: u64) -> u64 {
    bytes >> 20
}

// -------- Installing --------

/// Downloads, verifies, unpacks and proves one backend. `root` is
/// `Paths::llama_dir()`; `progress` is the CLI's stdout sink.
pub async fn setup(
    root: &Path,
    opts: &SetupOptions,
    loc: &Locale,
    mut progress: impl FnMut(&str),
) -> Result<Installed> {
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    check_platform(os, arch, loc)?;
    std::fs::create_dir_all(root).with_context(|| {
        loc.tf(
            "llamacpp.setup.mkdir",
            &[("path", &root.display().to_string())],
        )
    })?;

    let client = http_client(loc)?;
    let release = fetch_release(&client, opts.build.as_deref(), os, arch, loc).await?;
    let available = backends(&release, os, arch);
    let backend = available
        .iter()
        .find(|b| b.id == opts.backend)
        .ok_or_else(|| {
            let list = available
                .iter()
                .map(|b| b.id.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            anyhow::anyhow!(
                "{}",
                loc.tf(
                    "llamacpp.setup.unknown_backend",
                    &[
                        ("id", &opts.backend),
                        ("tag", &release.tag_name),
                        ("os", os),
                        ("arch", arch),
                        ("list", &list),
                    ],
                )
            )
        })?;

    // R4: a CUDA build without its runtime does not error — it loads no CUDA
    // device and runs on the CPU. Refuse rather than produce that.
    if opts.cudart && backend.cudart_missing() {
        bail!(
            "{}",
            loc.tf(
                "llamacpp.setup.cudart_missing",
                &[
                    ("id", &backend.id),
                    ("tag", &release.tag_name),
                    (
                        "name",
                        &cudart_name(
                            &backend.id,
                            os_token(os).unwrap_or(""),
                            arch_token(arch).unwrap_or("")
                        )
                    ),
                ],
            )
        );
    }

    let name = install_name(&backend.id, &release.tag_name);
    let dir = root.join(&name);
    let staging = root.join(format!(".tmp-{name}"));

    if dir.is_dir() && !opts.force {
        progress(&loc.tf(
            "llamacpp.setup.present",
            &[("path", &dir.display().to_string())],
        ));
        return probe(&backend.id, &release.tag_name, &dir, loc, &mut progress).await;
    }
    if opts.force {
        // A fresh download too: `--force` is the escape hatch from a bad asset,
        // so a kept `.part` would defeat it.
        let _ = std::fs::remove_dir_all(&staging);
    }
    std::fs::create_dir_all(&staging).with_context(|| {
        loc.tf(
            "llamacpp.setup.mkdir",
            &[("path", &staging.display().to_string())],
        )
    })?;

    let mut wanted: Vec<&Asset> = vec![&backend.asset];
    if opts.cudart
        && let Some(c) = &backend.cudart
    {
        wanted.push(c);
    }
    let total: u64 = wanted.iter().map(|a| a.size).sum();
    progress(&loc.tf(
        "llamacpp.setup.downloading",
        &[
            ("tag", &release.tag_name),
            ("backend", &backend.id),
            ("os", os),
            ("arch", arch),
            ("size", &mib(total).to_string()),
        ],
    ));

    // The unpack target is rebuilt from scratch every run; the `.part` files
    // beside it are not, which is what makes a resumed download possible.
    let unpacked = staging.join("unpacked");
    let _ = std::fs::remove_dir_all(&unpacked);
    for (i, asset) in wanted.iter().enumerate() {
        let archive = staging.join(&asset.name);
        fetch_verified(&client, asset, &archive, loc, &mut progress).await?;
        progress(&loc.tf("llamacpp.setup.extracting", &[("name", &asset.name)]));
        let raw = staging.join(format!("raw{i}"));
        let _ = std::fs::remove_dir_all(&raw);
        extract(&archive, &asset.name, &raw, loc)?;
        merge_payload(&raw, &unpacked, loc)?;
        let _ = std::fs::remove_dir_all(&raw);
    }

    let staged_binary = unpacked.join(server_binary_name());
    if !staged_binary.is_file() {
        bail!(
            "{}",
            loc.tf(
                "llamacpp.setup.binary_missing",
                &[
                    ("name", server_binary_name()),
                    ("path", &unpacked.display().to_string()),
                ],
            )
        );
    }
    // R5, and the only thing that proves the ~50-file library set is complete:
    // run it before it is moved into place, so a broken unpack never becomes an
    // install that `llama installed` would list.
    let version = first_line(&run_probe(&staged_binary, &["--version"], loc).await?);
    if let (Some(want), Some(got)) = (
        tag_build_number(&release.tag_name),
        version_build_number(&version),
    ) && want != got
    {
        bail!(
            "{}",
            loc.tf(
                "llamacpp.setup.build_mismatch",
                &[("got", &got.to_string()), ("expected", &want.to_string())],
            )
        );
    }

    if dir.exists() {
        std::fs::remove_dir_all(&dir).with_context(|| {
            loc.tf(
                "llamacpp.setup.remove_dir",
                &[("path", &dir.display().to_string())],
            )
        })?;
    }
    std::fs::rename(&unpacked, &dir).with_context(|| {
        loc.tf(
            "llamacpp.setup.rename",
            &[("path", &dir.display().to_string())],
        )
    })?;
    let _ = std::fs::remove_dir_all(&staging);

    progress(&loc.tf(
        "llamacpp.setup.installed",
        &[("path", &dir.display().to_string())],
    ));
    probe(&backend.id, &release.tag_name, &dir, loc, &mut progress).await
}

/// Runs the two model-free probes on an installed directory and reports what
/// they said (research §3.5). A GPU backend that found no device is named
/// out loud but does not fail: the files on disk are correct, what is missing
/// is on the host.
async fn probe(
    backend: &str,
    tag: &str,
    dir: &Path,
    loc: &Locale,
    progress: &mut impl FnMut(&str),
) -> Result<Installed> {
    let binary = dir.join(server_binary_name());
    if !binary.is_file() {
        bail!(
            "{}",
            loc.tf(
                "llamacpp.setup.binary_missing",
                &[
                    ("name", server_binary_name()),
                    ("path", &dir.display().to_string()),
                ],
            )
        );
    }
    // `--version` prints two lines (the version, then the compiler it was built
    // with); only the first is the answer.
    let version = first_line(&run_probe(&binary, &["--version"], loc).await?);
    let devices = parse_devices(&run_probe(&binary, &["--list-devices"], loc).await?);
    progress(&loc.tf("llamacpp.setup.version", &[("version", &version)]));
    if devices.is_empty() {
        if backend != "cpu" {
            progress(&loc.tf("llamacpp.setup.no_devices", &[("backend", backend)]));
        }
    } else {
        progress(&loc.tf(
            "llamacpp.setup.devices",
            &[("devices", &devices.join("; "))],
        ));
    }
    Ok(Installed {
        backend: backend.to_string(),
        tag: tag.to_string(),
        dir: dir.to_path_buf(),
        binary,
        version,
        devices,
    })
}

/// The first non-empty line of a probe's output, trimmed.
fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string()
}

/// Runs the binary with `args` and returns its output. `--version` writes to
/// **stderr** and `--list-devices` to stdout (measured), so both streams are
/// taken and the non-empty one is the answer.
async fn run_probe(binary: &Path, args: &[&str], loc: &Locale) -> Result<String> {
    let run = tokio::process::Command::new(binary)
        .args(args)
        .current_dir(binary.parent().unwrap_or(Path::new(".")))
        .output();
    let out = tokio::time::timeout(PROBE_TIMEOUT, run)
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "{}",
                loc.tf(
                    "llamacpp.setup.probe_timeout",
                    &[("path", &binary.display().to_string())],
                )
            )
        })?
        .with_context(|| {
            loc.tf(
                "llamacpp.setup.probe_failed",
                &[("path", &binary.display().to_string())],
            )
        })?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if stdout.trim().is_empty() {
        Ok(stderr)
    } else {
        Ok(stdout)
    }
}

/// `mindfork llama installed`: the builds already on disk, newest name first.
pub fn installed(root: &Path) -> Vec<Install> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out: Vec<Install> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // `.tmp-…` is an install in progress, not an install.
        if name.starts_with('.') {
            continue;
        }
        let Some((backend, tag)) = split_install_name(name) else {
            continue;
        };
        out.push(Install {
            backend: backend.to_string(),
            tag: tag.to_string(),
            bytes: dir_size(&path),
            binary_ok: path.join(server_binary_name()).is_file(),
            dir: path,
        });
    }
    out.sort_by(|a, b| (&a.backend, &a.tag).cmp(&(&b.backend, &b.tag)));
    out
}

/// Renders [`installed`] as printable lines.
pub fn render_installed(installs: &[Install], root: &Path, loc: &Locale) -> Vec<String> {
    if installs.is_empty() {
        return vec![loc.t("llamacpp.installed.none").to_string()];
    }
    let mut lines = vec![loc.tf(
        "llamacpp.installed.header",
        &[("path", &root.display().to_string())],
    )];
    let name_of = |i: &Install| {
        i.dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| install_name(&i.backend, &i.tag))
    };
    let width = installs
        .iter()
        .map(|i| name_of(i).chars().count())
        .max()
        .unwrap_or(0);
    for i in installs {
        let broken = if i.binary_ok {
            String::new()
        } else {
            format!("  ({})", loc.t("llamacpp.installed.broken"))
        };
        lines.push(format!(
            "  {:width$}  {:>5} MB{}",
            name_of(i),
            mib(i.bytes),
            broken,
            width = width
        ));
    }
    lines
}

/// Total bytes under `dir` (no crate does this and the tree is flat).
fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut total = 0;
    for entry in entries.flatten() {
        match entry.metadata() {
            Ok(m) if m.is_dir() => total += dir_size(&entry.path()),
            Ok(m) => total += m.len(),
            Err(_) => {}
        }
    }
    total
}

// -------- Download --------

/// Downloads `asset` to `dest` and verifies it against the release's own
/// sha256. Resumable: a `<dest>.part` left by an interrupted run is continued
/// with a `Range` request, and a digest that does not match after a resume
/// costs one retry from zero rather than the whole command.
async fn fetch_verified(
    client: &reqwest::Client,
    asset: &Asset,
    dest: &Path,
    loc: &Locale,
    progress: &mut impl FnMut(&str),
) -> Result<()> {
    let expected = asset
        .digest
        .as_deref()
        .and_then(digest_hex)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "{}",
                loc.tf("llamacpp.setup.no_digest", &[("name", &asset.name)])
            )
        })?;

    if dest.is_file()
        && std::fs::metadata(dest).map(|m| m.len()).ok() == Some(asset.size)
        && verify_file(dest, expected, &asset.name, loc).await.is_ok()
    {
        progress(&loc.tf("llamacpp.setup.asset_present", &[("name", &asset.name)]));
        return Ok(());
    }

    let part = PathBuf::from(format!("{}.part", dest.display()));
    let resumable = part.is_file();
    let first = stream_to_part(client, asset, &part, resumable, expected, loc, progress).await;
    if let Err(err) = first {
        if !resumable {
            return Err(err);
        }
        progress(&loc.tf("llamacpp.setup.retry", &[("name", &asset.name)]));
        let _ = std::fs::remove_file(&part);
        stream_to_part(client, asset, &part, false, expected, loc, progress).await?;
    }
    let _ = std::fs::remove_file(dest);
    std::fs::rename(&part, dest).with_context(|| {
        loc.tf(
            "llamacpp.setup.rename",
            &[("path", &dest.display().to_string())],
        )
    })
}

/// One download attempt into `part`, hashing as the bytes pass and verifying at
/// the end. With `resume`, continues an existing part through a `Range` request
/// — seeding the hasher from what is already on disk — and falls back to a full
/// download if the server ignores the range.
async fn stream_to_part(
    client: &reqwest::Client,
    asset: &Asset,
    part: &Path,
    resume: bool,
    expected: &str,
    loc: &Locale,
    progress: &mut impl FnMut(&str),
) -> Result<()> {
    let have = if resume {
        std::fs::metadata(part).map(|m| m.len()).unwrap_or(0)
    } else {
        0
    };
    let have = if have >= asset.size { 0 } else { have };

    let mut req = client.get(&asset.browser_download_url);
    if have > 0 {
        req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
    }
    let resp = req
        .send()
        .await
        .with_context(|| {
            loc.tf(
                "llamacpp.setup.request",
                &[("url", &asset.browser_download_url)],
            )
        })?
        .error_for_status()
        .with_context(|| {
            loc.tf(
                "llamacpp.setup.download",
                &[("url", &asset.browser_download_url)],
            )
        })?;
    // The server may ignore the range and answer 200 with the whole body.
    let partial = resp.status() == reqwest::StatusCode::PARTIAL_CONTENT;
    let (mut hasher, mut done, append) = if have > 0 && partial {
        progress(&loc.tf(
            "llamacpp.setup.resuming",
            &[("done", &mib(have).to_string())],
        ));
        (hash_of(part, loc).await?, have, true)
    } else {
        (Sha256::new(), 0, false)
    };

    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append)
        .open(part)
        .await
        .with_context(|| {
            loc.tf(
                "llamacpp.setup.create_file",
                &[("path", &part.display().to_string())],
            )
        })?;
    let mut next_report = done + PROGRESS_STEP;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.with_context(|| loc.t("llamacpp.setup.read_stream").to_string())?;
        hasher.update(&chunk);
        file.write_all(&chunk)
            .await
            .with_context(|| loc.t("llamacpp.setup.write_file").to_string())?;
        done += chunk.len() as u64;
        if done >= next_report {
            progress(&loc.tf(
                "llamacpp.setup.progress_bytes",
                &[
                    ("done", &mib(done).to_string()),
                    ("total", &mib(asset.size).to_string()),
                ],
            ));
            next_report = done + PROGRESS_STEP;
        }
    }
    file.flush()
        .await
        .with_context(|| loc.t("llamacpp.setup.flush").to_string())?;
    verify_sha256(&hasher.finalize(), expected, &asset.name, loc)
}

/// sha256 of a file already on disk, streamed (the sandbox's 64 KB loop).
async fn hash_of(path: &Path, loc: &Locale) -> Result<Sha256> {
    use tokio::io::AsyncReadExt;

    let mut file = tokio::fs::File::open(path).await.with_context(|| {
        loc.tf(
            "llamacpp.setup.open_file",
            &[("path", &path.display().to_string())],
        )
    })?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buf)
            .await
            .with_context(|| loc.t("llamacpp.setup.read_file").to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(hasher)
}

/// Verifies a finished file against `expected`.
async fn verify_file(path: &Path, expected: &str, label: &str, loc: &Locale) -> Result<()> {
    let hasher = hash_of(path, loc).await?;
    verify_sha256(&hasher.finalize(), expected, label, loc)
}

/// Compares a raw digest against the expected hex; the error names the asset.
fn verify_sha256(digest: &[u8], expected: &str, label: &str, loc: &Locale) -> Result<()> {
    let got = hex_lower(digest);
    if got.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        bail!(
            "{}",
            loc.tf(
                "llamacpp.setup.sha_mismatch",
                &[("name", label), ("expected", expected), ("got", &got)],
            )
        )
    }
}

/// Bytes → a hex string (lowercase), without the `hex` crate (ADR 0008).
fn hex_lower(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

// -------- Unpacking --------

/// Unpacks `archive` into `dest`, dispatching on the **name's** extension.
fn extract(archive: &Path, name: &str, dest: &Path, loc: &Locale) -> Result<()> {
    std::fs::create_dir_all(dest).with_context(|| {
        loc.tf(
            "llamacpp.setup.mkdir",
            &[("path", &dest.display().to_string())],
        )
    })?;
    match Ext::of(name) {
        Some(Ext::TarGz) => extract_targz(archive, dest, loc),
        Some(Ext::Zip) => extract_zip(archive, dest, loc),
        None => bail!(
            "{}",
            loc.tf("llamacpp.setup.unknown_archive", &[("name", name)])
        ),
    }
}

/// tar.gz: `tar-rs` preserves the `0755` bits and creates the tarball's
/// symlinks (`libllama.so -> libllama.so.0`), and guards path containment
/// itself.
fn extract_targz(archive: &Path, dest: &Path, loc: &Locale) -> Result<()> {
    let file = std::fs::File::open(archive).with_context(|| {
        loc.tf(
            "llamacpp.setup.open_archive",
            &[("path", &archive.display().to_string())],
        )
    })?;
    let gz = flate2::read::GzDecoder::new(std::io::BufReader::new(file));
    tar::Archive::new(gz).unpack(dest).with_context(|| {
        loc.tf(
            "llamacpp.setup.extract_to",
            &[("path", &dest.display().to_string())],
        )
    })
}

/// zip: zip-slip guarded with `enclosed_name` (as in `features::backup`).
///
/// The mode is set explicitly on unix. Upstream ships Windows zips and Linux
/// tarballs today, but it shipped **Linux zips** until 2025 (research §3.3) and
/// a zip carries no mode `std::fs::write` would apply — an executable extracted
/// that way would land without its `+x` bit.
fn extract_zip(archive: &Path, dest: &Path, loc: &Locale) -> Result<()> {
    let file = std::fs::File::open(archive).with_context(|| {
        loc.tf(
            "llamacpp.setup.open_archive",
            &[("path", &archive.display().to_string())],
        )
    })?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file)).with_context(|| {
        loc.tf(
            "llamacpp.setup.open_archive",
            &[("path", &archive.display().to_string())],
        )
    })?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let Some(rel) = entry.enclosed_name() else {
            bail!(
                "{}",
                loc.tf("llamacpp.setup.unsafe_entry", &[("name", entry.name())])
            );
        };
        let out = dest.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out_file = std::fs::File::create(&out).with_context(|| {
            loc.tf(
                "llamacpp.setup.create_file",
                &[("path", &out.display().to_string())],
            )
        })?;
        std::io::copy(&mut entry, &mut out_file)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = entry.unix_mode().unwrap_or(0o755);
            let _ = std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode));
        }
    }
    Ok(())
}

/// Moves the payload of a freshly unpacked archive into `dest`, collapsing a
/// single shared root directory if there is one.
///
/// The two layouts differ by exactly that: the Windows zip is flat, the Linux
/// tarball wraps everything in `llama-<tag>/` (research §3.4). Deciding after
/// the fact — "one directory and nothing else" — needs no second pass over the
/// stream and treats both the same way.
fn merge_payload(raw: &Path, dest: &Path, loc: &Locale) -> Result<()> {
    let payload = single_child_dir(raw).unwrap_or_else(|| raw.to_path_buf());
    std::fs::create_dir_all(dest).with_context(|| {
        loc.tf(
            "llamacpp.setup.mkdir",
            &[("path", &dest.display().to_string())],
        )
    })?;
    let entries = std::fs::read_dir(&payload).with_context(|| {
        loc.tf(
            "llamacpp.setup.read_dir",
            &[("path", &payload.display().to_string())],
        )
    })?;
    for entry in entries.flatten() {
        let to = dest.join(entry.file_name());
        let _ = std::fs::remove_file(&to);
        std::fs::rename(entry.path(), &to).with_context(|| {
            loc.tf(
                "llamacpp.setup.rename",
                &[("path", &to.display().to_string())],
            )
        })?;
    }
    Ok(())
}

/// The one directory `dir` contains, when that is all it contains.
fn single_child_dir(dir: &Path) -> Option<PathBuf> {
    let mut only: Option<PathBuf> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        if only.is_some() || !entry.path().is_dir() {
            return None;
        }
        only = Some(entry.path());
    }
    only
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::i18n::{Lang, locale};

    /// The 27 assets of build `b10883` (2026-09-09), verbatim. Every derivation
    /// test runs against the real set rather than a hand-written idealisation of
    /// it — research §3.2.
    const B10883: &[&str] = &[
        "cudart-llama-bin-win-cuda-12.4-x64.zip",
        "cudart-llama-bin-win-cuda-13.3-x64.zip",
        "cudart-llama-bin-win-cuda-13.4-arm64.zip",
        "llama-b10883-bin-android-arm64.tar.gz",
        "llama-b10883-bin-macos-arm64.tar.gz",
        "llama-b10883-bin-macos-x64.tar.gz",
        "llama-b10883-bin-ubuntu-arm64.tar.gz",
        "llama-b10883-bin-ubuntu-openvino-2026.3.1-x64.tar.gz",
        "llama-b10883-bin-ubuntu-rocm-10.0-x64.tar.gz",
        "llama-b10883-bin-ubuntu-s390x.tar.gz",
        "llama-b10883-bin-ubuntu-sycl-fp16-x64.tar.gz",
        "llama-b10883-bin-ubuntu-sycl-fp32-x64.tar.gz",
        "llama-b10883-bin-ubuntu-vulkan-arm64.tar.gz",
        "llama-b10883-bin-ubuntu-vulkan-x64.tar.gz",
        "llama-b10883-bin-ubuntu-x64.tar.gz",
        "llama-b10883-bin-win-cpu-arm64.zip",
        "llama-b10883-bin-win-cpu-x64.zip",
        "llama-b10883-bin-win-cuda-12.4-x64.zip",
        "llama-b10883-bin-win-cuda-13.3-x64.zip",
        "llama-b10883-bin-win-cuda-13.4-arm64.zip",
        "llama-b10883-bin-win-opencl-adreno-arm64.zip",
        "llama-b10883-bin-win-openvino-2026.3.1-x64.zip",
        "llama-b10883-bin-win-rocm-10.0-x64.zip",
        "llama-b10883-bin-win-sycl-x64.zip",
        "llama-b10883-bin-win-vulkan-x64.zip",
        "llama-b10883-ui.tar.gz",
        "llama-b10883-xcframework.zip",
    ];

    /// Build `b9000` (2026-05-02): the AMD backend was still called
    /// `hip-radeon`, and the openEuler rows put something other than an OS in
    /// the first token and something other than an arch in the last.
    const B9000: &[&str] = &[
        "cudart-llama-bin-win-cuda-12.4-x64.zip",
        "cudart-llama-bin-win-cuda-13.1-x64.zip",
        "llama-b9000-bin-310p-openEuler-aarch64.tar.gz",
        "llama-b9000-bin-310p-openEuler-x86.tar.gz",
        "llama-b9000-bin-910b-openEuler-aarch64-aclgraph.tar.gz",
        "llama-b9000-bin-910b-openEuler-x86-aclgraph.tar.gz",
        "llama-b9000-bin-macos-arm64-kleidiai.tar.gz",
        "llama-b9000-bin-macos-arm64.tar.gz",
        "llama-b9000-bin-ubuntu-x64.tar.gz",
        "llama-b9000-bin-win-cpu-x64.zip",
        "llama-b9000-bin-win-cuda-12.4-x64.zip",
        "llama-b9000-bin-win-hip-radeon-x64.zip",
        "llama-b9000-bin-win-vulkan-x64.zip",
        "llama-b9000-xcframework.zip",
    ];

    /// Build `b6000` (2025-07-27): Linux and macOS shipped as **`.zip`**.
    const B6000: &[&str] = &[
        "cudart-llama-bin-win-cuda-12.4-x64.zip",
        "llama-b6000-bin-macos-x64.zip",
        "llama-b6000-bin-ubuntu-vulkan-x64.zip",
        "llama-b6000-bin-ubuntu-x64.zip",
        "llama-b6000-bin-win-cpu-x64.zip",
        "llama-b6000-bin-win-cuda-12.4-x64.zip",
        "llama-b6000-bin-win-hip-radeon-x64.zip",
    ];

    fn release(tag: &str, names: &[&str]) -> Release {
        Release {
            tag_name: tag.to_string(),
            published_at: Some("2026-09-09T17:29:28Z".to_string()),
            assets: names
                .iter()
                .enumerate()
                .map(|(i, n)| Asset {
                    name: (*n).to_string(),
                    size: (i as u64 + 1) << 20,
                    digest: Some(format!("sha256:{}", "0".repeat(64))),
                    browser_download_url: format!("https://example.invalid/{n}"),
                })
                .collect(),
        }
    }

    fn ids(release: &Release, os: &str, arch: &str) -> Vec<String> {
        backends(release, os, arch)
            .into_iter()
            .map(|b| b.id)
            .collect()
    }

    #[test]
    fn platform_tokens_map_only_what_upstream_publishes() {
        assert_eq!(os_token("windows"), Some("win"));
        assert_eq!(os_token("linux"), Some("ubuntu"));
        assert_eq!(os_token("macos"), Some("macos"));
        assert_eq!(os_token("freebsd"), None);
        assert_eq!(arch_token("x86_64"), Some("x64"));
        assert_eq!(arch_token("aarch64"), Some("arm64"));
        assert_eq!(arch_token("riscv64"), None);
    }

    #[test]
    fn windows_x64_backends_of_the_newest_build() {
        assert_eq!(
            ids(&release("b10883", B10883), "windows", "x86_64"),
            [
                "cpu",
                "cuda-12.4",
                "cuda-13.3",
                "openvino-2026.3.1",
                "rocm-10.0",
                "sycl",
                "vulkan",
            ]
        );
    }

    /// The Linux CPU build has **no backend token at all**
    /// (`llama-b10883-bin-ubuntu-x64.tar.gz`); an empty middle means `cpu`, or
    /// Linux gets a backend named "" that nobody can type.
    #[test]
    fn linux_x64_reads_an_empty_middle_as_cpu() {
        let got = ids(&release("b10883", B10883), "linux", "x86_64");
        assert!(got.contains(&"cpu".to_string()), "{got:?}");
        assert_eq!(
            got,
            [
                "cpu",
                "openvino-2026.3.1",
                "rocm-10.0",
                "sycl-fp16",
                "sycl-fp32",
                "vulkan",
            ]
        );
    }

    #[test]
    fn arm64_rows_are_kept_for_arm64_hosts_only() {
        assert_eq!(
            ids(&release("b10883", B10883), "windows", "aarch64"),
            ["cpu", "cuda-13.4", "opencl-adreno"]
        );
        assert_eq!(
            ids(&release("b10883", B10883), "linux", "aarch64"),
            ["cpu", "vulkan"]
        );
    }

    /// Everything that is not a server build for this platform is skipped by the
    /// shape alone — no per-name special case exists or is needed.
    #[test]
    fn non_server_and_foreign_assets_are_skipped() {
        for name in [
            "llama-b10883-ui.tar.gz",
            "llama-b10883-xcframework.zip",
            "llama-b10883-bin-android-arm64.tar.gz",
            "llama-b10883-bin-ubuntu-s390x.tar.gz",
            "cudart-llama-bin-win-cuda-12.4-x64.zip",
        ] {
            assert_eq!(backend_of(name, "b10883", "ubuntu", "x64"), None, "{name}");
            assert_eq!(backend_of(name, "b10883", "win", "x64"), None, "{name}");
        }
        for name in [
            "llama-b9000-bin-310p-openEuler-x86.tar.gz",
            "llama-b9000-bin-910b-openEuler-x86-aclgraph.tar.gz",
            "llama-b9000-bin-macos-arm64-kleidiai.tar.gz",
        ] {
            assert_eq!(backend_of(name, "b9000", "ubuntu", "x64"), None, "{name}");
            assert_eq!(backend_of(name, "b9000", "macos", "arm64"), None, "{name}");
        }
    }

    /// A backend upstream renames must appear without a mindfork release (R1):
    /// at `b9000` the AMD build was `hip-radeon`, today it is `rocm-10.0`, and
    /// nothing in this module knows either name.
    #[test]
    fn a_renamed_backend_still_comes_through() {
        assert_eq!(
            ids(&release("b9000", B9000), "windows", "x86_64"),
            ["cpu", "cuda-12.4", "hip-radeon", "vulkan"]
        );
    }

    /// The archive kind is read off the name: in 2025 the Linux builds were
    /// zips, and assuming `.tar.gz` from the OS would have unpacked nothing.
    #[test]
    fn the_extension_comes_from_the_name_not_the_platform() {
        assert_eq!(
            backend_of("llama-b6000-bin-ubuntu-x64.zip", "b6000", "ubuntu", "x64"),
            Some(("cpu".to_string(), Ext::Zip))
        );
        assert_eq!(
            backend_of(
                "llama-b10883-bin-ubuntu-x64.tar.gz",
                "b10883",
                "ubuntu",
                "x64"
            ),
            Some(("cpu".to_string(), Ext::TarGz))
        );
        assert_eq!(
            ids(&release("b6000", B6000), "linux", "x86_64"),
            ["cpu", "vulkan"]
        );
    }

    #[test]
    fn cuda_backends_are_paired_with_their_runtime() {
        let rel = release("b10883", B10883);
        let found = backends(&rel, "windows", "x86_64");
        let cuda = found.iter().find(|b| b.id == "cuda-12.4").unwrap();
        assert_eq!(
            cuda.cudart.as_ref().map(|a| a.name.as_str()),
            Some("cudart-llama-bin-win-cuda-12.4-x64.zip")
        );
        assert!(cuda.needs_cudart() && !cuda.cudart_missing());
        assert_eq!(
            cuda.download_size(),
            cuda.asset.size + cuda.cudart.as_ref().unwrap().size
        );
        let vulkan = found.iter().find(|b| b.id == "vulkan").unwrap();
        assert!(!vulkan.needs_cudart() && vulkan.cudart.is_none());
    }

    /// A `cuda-*` build whose runtime archive is absent is a half-install that
    /// runs on the CPU without saying so — the case R4 exists to refuse.
    #[test]
    fn a_cuda_backend_without_a_runtime_is_flagged() {
        let names: Vec<&str> = B10883
            .iter()
            .copied()
            .filter(|n| *n != "cudart-llama-bin-win-cuda-12.4-x64.zip")
            .collect();
        let rel = release("b10883", &names);
        let found = backends(&rel, "windows", "x86_64");
        let cuda = found.iter().find(|b| b.id == "cuda-12.4").unwrap();
        assert!(cuda.cudart_missing());
        assert!(
            !found
                .iter()
                .find(|b| b.id == "cuda-13.3")
                .unwrap()
                .cudart_missing()
        );
    }

    #[test]
    fn an_unsupported_platform_yields_nothing_rather_than_a_guess() {
        assert!(backends(&release("b10883", B10883), "freebsd", "x86_64").is_empty());
        assert!(backends(&release("b10883", B10883), "linux", "riscv64").is_empty());
        let loc = locale(Lang::En);
        assert!(check_platform("freebsd", "x86_64", loc).is_err());
        assert!(check_platform("linux", "x86_64", loc).is_ok());
    }

    #[test]
    fn install_names_round_trip_through_a_dashed_backend() {
        assert_eq!(install_name("cuda-12.4", "b10883"), "cuda-12.4-b10883");
        assert_eq!(
            split_install_name("cuda-12.4-b10883"),
            Some(("cuda-12.4", "b10883"))
        );
        assert_eq!(split_install_name("cpu-b10883"), Some(("cpu", "b10883")));
        assert_eq!(split_install_name("nodash"), None);
    }

    #[test]
    fn only_a_well_formed_sha256_digest_is_accepted() {
        let hex = "8c79a9b226de4b3cacfd1f83d24f962d0773be79f1e7b75c6af4ded7e32ae1d6";
        assert_eq!(digest_hex(&format!("sha256:{hex}")), Some(hex));
        assert_eq!(digest_hex(&format!("sha512:{hex}")), None);
        assert_eq!(digest_hex("sha256:abc"), None);
        assert_eq!(digest_hex(&format!("sha256:{}", "z".repeat(64))), None);
    }

    #[test]
    fn the_installed_build_is_compared_with_the_tag() {
        assert_eq!(tag_build_number("b10883"), Some(10883));
        assert_eq!(tag_build_number("v0.4.0"), None);
        assert_eq!(
            version_build_number("version: 0.3.0-dev (build 10807, commit 163a40796)"),
            Some(10807)
        );
        assert_eq!(version_build_number("built with MSVC 19.51 for x64"), None);
    }

    #[test]
    fn list_devices_is_read_as_a_list_or_as_none() {
        assert!(parse_devices("Available devices:\n  (none)\n").is_empty());
        assert_eq!(
            parse_devices("Available devices:\n  CUDA0: RTX 4090 (24564 MiB)\n\n"),
            ["CUDA0: RTX 4090 (24564 MiB)"]
        );
        assert!(parse_devices("").is_empty());
    }

    #[test]
    fn sha_mismatch_error_is_localized() {
        let bad = [0u8; 32];
        let en = verify_sha256(&bad, &"a".repeat(64), "asset.zip", locale(Lang::En))
            .unwrap_err()
            .to_string();
        let ru = verify_sha256(&bad, &"a".repeat(64), "asset.zip", locale(Lang::Ru))
            .unwrap_err()
            .to_string();
        assert!(
            !en.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c)),
            "{en}"
        );
        assert!(
            ru.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c)),
            "{ru}"
        );
        assert!(en.contains("asset.zip"));
    }

    #[test]
    fn the_backends_listing_marks_what_is_already_installed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("vulkan-b10883")).unwrap();
        let listing = Listing {
            tag: "b10883".to_string(),
            date: "2026-09-09".to_string(),
            backends: backends(&release("b10883", B10883), std::env::consts::OS, "x86_64"),
        };
        let lines = render_backends(&listing, dir.path(), locale(Lang::En));
        assert!(lines[0].contains("b10883") && lines[0].contains("2026-09-09"));
        let vulkan = lines.iter().find(|l| l.contains("vulkan")).unwrap();
        assert!(vulkan.contains("MB"), "{vulkan}");
        assert!(
            vulkan.contains(locale(Lang::En).t("llamacpp.backends.installed")),
            "{vulkan}"
        );
    }

    #[test]
    fn installed_lists_finished_directories_and_skips_staging() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("cuda-12.4-b10883");
        std::fs::create_dir_all(&good).unwrap();
        std::fs::write(good.join(server_binary_name()), b"x").unwrap();
        std::fs::create_dir_all(dir.path().join("cpu-b10871")).unwrap();
        std::fs::create_dir_all(dir.path().join(".tmp-cpu-b10883")).unwrap();
        let found = installed(dir.path());
        assert_eq!(
            found
                .iter()
                .map(|i| i.dir.file_name().unwrap().to_string_lossy().to_string())
                .collect::<Vec<_>>(),
            ["cpu-b10871", "cuda-12.4-b10883"]
        );
        assert!(!found[0].binary_ok, "an empty directory has no binary");
        assert!(found[1].binary_ok);
        assert_eq!(found[1].backend, "cuda-12.4");
        assert_eq!(found[1].tag, "b10883");
        let lines = render_installed(&found, dir.path(), locale(Lang::En));
        assert!(lines.iter().any(|l| l.contains("cuda-12.4-b10883")));
        assert_eq!(
            render_installed(&[], dir.path(), locale(Lang::En)),
            [locale(Lang::En).t("llamacpp.installed.none")]
        );
    }

    // -------- Unpacking --------

    fn zip_with(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write;
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            for (name, body) in entries {
                w.start_file::<_, ()>(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                w.write_all(body).unwrap();
            }
            w.finish().unwrap();
        }
        buf
    }

    fn targz_with(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        {
            let mut tar = tar::Builder::new(&mut gz);
            for (name, body) in entries {
                let mut header = tar::Header::new_gnu();
                header.set_size(body.len() as u64);
                header.set_mode(0o755);
                header.set_cksum();
                tar.append_data(&mut header, name, *body).unwrap();
            }
            tar.finish().unwrap();
        }
        gz.finish().unwrap()
    }

    /// The Windows zip is flat and the Linux tarball wraps everything in
    /// `llama-<tag>/`; one rule — collapse a single shared root — covers both.
    #[test]
    fn a_single_root_directory_is_collapsed_for_both_layouts() {
        let loc = locale(Lang::En);
        for (name, bytes) in [
            (
                "llama-b1-bin-win-cpu-x64.zip",
                zip_with(&[("llama-server.exe", b"a"), ("llama.dll", b"b")]),
            ),
            (
                "llama-b1-bin-ubuntu-x64.tar.gz",
                targz_with(&[
                    ("llama-b1/llama-server", b"a"),
                    ("llama-b1/libllama.so", b"b"),
                ]),
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let archive = dir.path().join(name);
            std::fs::write(&archive, &bytes).unwrap();
            let raw = dir.path().join("raw");
            extract(&archive, name, &raw, loc).unwrap();
            let out = dir.path().join("unpacked");
            merge_payload(&raw, &out, loc).unwrap();
            let mut got: Vec<String> = std::fs::read_dir(&out)
                .unwrap()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect();
            got.sort();
            assert_eq!(got.len(), 2, "{name}: {got:?}");
            assert!(got.iter().any(|g| g.starts_with("llama-server")), "{got:?}");
        }
    }

    #[test]
    fn a_flat_archive_is_left_alone() {
        let loc = locale(Lang::En);
        let dir = tempfile::tempdir().unwrap();
        let name = "cudart-llama-bin-win-cuda-12.4-x64.zip";
        let archive = dir.path().join(name);
        std::fs::write(
            &archive,
            zip_with(&[("cudart64_12.dll", b"a"), ("cublas64_12.dll", b"b")]),
        )
        .unwrap();
        let raw = dir.path().join("raw");
        extract(&archive, name, &raw, loc).unwrap();
        let out = dir.path().join("unpacked");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("llama-server.exe"), b"x").unwrap();
        merge_payload(&raw, &out, loc).unwrap();
        assert!(out.join("cudart64_12.dll").is_file());
        assert!(
            out.join("llama-server.exe").is_file(),
            "the build stays put"
        );
    }

    #[test]
    fn an_unknown_archive_kind_is_refused() {
        let loc = locale(Lang::En);
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("build.7z");
        std::fs::write(&archive, b"x").unwrap();
        assert!(extract(&archive, "build.7z", &dir.path().join("raw"), loc).is_err());
    }

    /// Zip-slip, guarded the way `features::backup` guards it.
    #[test]
    fn an_escaping_zip_entry_is_refused() {
        let loc = locale(Lang::En);
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("evil.zip");
        std::fs::write(&archive, zip_with(&[("../escaped.txt", b"x")])).unwrap();
        let err = extract(&archive, "evil.zip", &dir.path().join("raw"), loc).unwrap_err();
        assert!(err.to_string().contains("escaped.txt"), "{err}");
        assert!(!dir.path().join("escaped.txt").exists());
    }

    // -------- Live (needs the network) --------

    /// The shape upstream publishes is a contract this module reads rather than
    /// pins, so this is the test that fails instead of a user when it changes:
    /// the newest build must still name a `cpu` backend for this platform.
    #[test]
    #[ignore = "queries the GitHub releases API"]
    fn live_the_newest_build_still_names_a_cpu_backend() {
        let loc = locale(Lang::En);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let listing = rt.block_on(list_backends(None, loc)).unwrap();
        let ids: Vec<&str> = listing.backends.iter().map(|b| b.id.as_str()).collect();
        println!("build {} ({}): {ids:?}", listing.tag, listing.date);
        assert!(tag_build_number(&listing.tag).is_some(), "{}", listing.tag);
        assert!(ids.contains(&"cpu"), "{ids:?}");
        for b in &listing.backends {
            assert!(
                b.asset.digest.as_deref().and_then(digest_hex).is_some(),
                "{} has no usable digest",
                b.asset.name
            );
            assert!(!b.cudart_missing(), "{} has no CUDA runtime", b.id);
        }
    }

    /// The whole path end to end on the cheapest asset (~18 MB): resolve,
    /// download, verify, unpack, and prove the binary reports the tag's build.
    #[test]
    #[ignore = "downloads the ~18 MB cpu build from GitHub"]
    fn live_install_cpu_into_a_tempdir() {
        let loc = locale(Lang::En);
        let dir = tempfile::tempdir().unwrap();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let out = rt
            .block_on(setup(
                dir.path(),
                &SetupOptions {
                    backend: "cpu".to_string(),
                    build: None,
                    force: false,
                    cudart: true,
                },
                loc,
                |m| println!("{m}"),
            ))
            .unwrap();
        assert!(out.binary.is_file(), "{:?}", out.binary);
        assert_eq!(
            version_build_number(&out.version),
            tag_build_number(&out.tag),
            "{}",
            out.version
        );
        let found = installed(dir.path());
        assert_eq!(found.len(), 1);
        assert!(found[0].binary_ok && found[0].bytes > 0);
        assert!(
            !dir.path()
                .join(format!(".tmp-{}", install_name("cpu", &out.tag)))
                .exists(),
            "staging is removed on success"
        );
    }
}
