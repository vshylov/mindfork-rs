//! The live tests of `features::llama_setup` (they need the network). Kept
//! under a `tests/` directory so that coverage — which runs without
//! `--ignored` — leaves them out, as it does the orchestrator's live suite.

use super::*;
use crate::shared::i18n::{Lang, locale};

/// The backend every build carries for this platform: `cpu`, except on a
/// Mac, where llama.cpp publishes only `metal` — measured on the rented
/// Mac, where both tests below failed asking for a `cpu` build
/// (docs/research/macos.md §14.2, defect D7).
const BASE_BACKEND: &str = if cfg!(target_os = "macos") {
    "metal"
} else {
    "cpu"
};

/// The shape upstream publishes is a contract this module reads rather than
/// pins, so this is the test that fails instead of a user when it changes:
/// the newest build must still name this platform's base backend.
#[test]
#[ignore = "queries the GitHub releases API"]
fn live_the_newest_build_still_names_the_base_backend() {
    let loc = locale(Lang::En);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let listing = rt.block_on(list_backends(None, loc)).unwrap();
    let ids: Vec<&str> = listing.backends.iter().map(|b| b.id.as_str()).collect();
    println!("build {} ({}): {ids:?}", listing.tag, listing.date);
    assert!(tag_build_number(&listing.tag).is_some(), "{}", listing.tag);
    assert!(ids.contains(&BASE_BACKEND), "{ids:?}");
    for b in &listing.backends {
        assert!(
            b.asset.digest.as_deref().and_then(digest_hex).is_some(),
            "{} has no usable digest",
            b.asset.name
        );
        assert!(!b.cudart_missing(), "{} has no CUDA runtime", b.id);
    }
}

/// The whole path end to end on the cheapest asset (the base backend,
/// 11–18 MB): resolve, download, verify, unpack, and prove the binary
/// reports the tag's build.
#[test]
#[ignore = "downloads the base build (cpu; metal on a Mac) from GitHub"]
fn live_install_the_base_backend_into_a_tempdir() {
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
                backend: BASE_BACKEND.to_string(),
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
            .join(format!(".tmp-{}", install_name(BASE_BACKEND, &out.tag)))
            .exists(),
        "staging is removed on success"
    );
}
