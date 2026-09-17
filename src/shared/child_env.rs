//! What a child process the **model** drives does not inherit
//! (docs/research/safe-defaults.md D5).
//!
//! Two children run code the model wrote or edited: the local Python interpreter, and a
//! code-workspace command whose build script the model may have just changed. They ran
//! with the app's whole environment, so any API key exported in the shell that started
//! mindfork was one `os.environ` away. The measurement behind the choice: an allowlist
//! ran every child class here but silently drops what nobody can list in advance
//! (`CARGO_HOME`, `JAVA_HOME`, `VIRTUAL_ENV`, proxies, certificates), while removing the
//! credential-shaped names broke nothing measured.
//!
//! Two limits, stated rather than papered over: a secret under a name this does not
//! recognise (`DATABASE_URL` with a password in it) still travels, and **no** filter stops
//! a child from reading credential *files* — it runs as the user. This narrows what an
//! injected model gets for free; it is not a boundary.
//!
//! `llama-server` and MCP servers deliberately keep the whole environment: the user chose
//! that software as they would from a shell, and spec §9.6 promises an MCP server that a
//! variable set in the app's own environment reaches it without being listed (D6).

use std::ffi::OsString;

/// Name segments that mean "this is a credential". Compared case-insensitively against
/// the `_`-separated parts of a variable's name, so `HF_TOKEN` and `AWS_SESSION_TOKEN`
/// match while `TOKENIZERS_PARALLELISM` — a real variable that is not a secret — does not.
const CREDENTIAL_SEGMENTS: &[&str] = &[
    "KEY",
    "KEYS",
    "APIKEY",
    "TOKEN",
    "TOKENS",
    "SECRET",
    "SECRETS",
    "PASSWORD",
    "PASSWD",
    "PAT",
    "CREDENTIAL",
    "CREDENTIALS",
];

/// Whether a variable's name looks like a credential's.
fn is_credential_name(name: &str) -> bool {
    name.split(['_', '-']).any(|part| {
        CREDENTIAL_SEGMENTS
            .iter()
            .any(|seg| part.eq_ignore_ascii_case(seg))
    })
}

/// The variables to remove from a model-driven child's environment: every credential-shaped
/// name in this process's environment, plus every name `named` lists — the variables the
/// user's own settings point at for a key, which need match no pattern (`MY_OPENAI`).
///
/// Returns names rather than taking a command, because `std` and `tokio` commands are two
/// types with one `env_remove`.
pub fn credential_vars(named: &[String]) -> Vec<OsString> {
    let mut out: Vec<OsString> = std::env::vars_os()
        .map(|(k, _)| k)
        .filter(|k| is_credential_name(&k.to_string_lossy()))
        .collect();
    for name in named {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let name = OsString::from(name);
        if !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shapes this is for, and the ones it must leave alone — a filter that removed
    /// everything would pass the first half alone (lessons §2).
    #[test]
    fn credential_names_are_recognised_by_segment() {
        for name in [
            "OPENAI_API_KEY",
            "openai_api_key",
            "HF_TOKEN",
            "GITHUB_TOKEN",
            "AWS_SECRET_ACCESS_KEY",
            "AWS_SESSION_TOKEN",
            "DB_PASSWORD",
            "GH_PAT",
            "GOOGLE_APPLICATION_CREDENTIALS",
            "MINDFORK-BACKUP-PASSWORD",
        ] {
            assert!(is_credential_name(name), "{name} should be removed");
        }
        for name in [
            "TOKENIZERS_PARALLELISM",
            "PATH",
            "CARGO_HOME",
            "JAVA_HOME",
            "VIRTUAL_ENV",
            "HTTPS_PROXY",
            "SSH_AUTH_SOCK",
            "KEYBOARD_LAYOUT",
            "PATTERN",
        ] {
            assert!(!is_credential_name(name), "{name} must stay");
        }
    }

    /// A name the settings point at is removed whatever it is called, and is not listed
    /// twice when it also matches the pattern.
    #[test]
    fn named_variables_are_added_once() {
        unsafe {
            std::env::set_var("MINDFORK_TEST_PLAIN_NAME", "x");
            std::env::set_var("MINDFORK_TEST_SECRET_KEY", "x");
        }
        let vars = credential_vars(&[
            "MINDFORK_TEST_PLAIN_NAME".into(),
            "MINDFORK_TEST_SECRET_KEY".into(),
            "  ".into(),
        ]);
        let count = |needle: &str| {
            vars.iter()
                .filter(|v| v.to_string_lossy() == needle)
                .count()
        };
        assert_eq!(count("MINDFORK_TEST_PLAIN_NAME"), 1);
        assert_eq!(count("MINDFORK_TEST_SECRET_KEY"), 1, "{vars:?}");
        assert!(!vars.iter().any(|v| v.is_empty()));
        unsafe {
            std::env::remove_var("MINDFORK_TEST_PLAIN_NAME");
            std::env::remove_var("MINDFORK_TEST_SECRET_KEY");
        }
    }
}
