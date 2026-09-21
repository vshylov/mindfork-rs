//! `mindfork setup` — the settings half of one-command provisioning
//! ([docs/research/cloud-provisioning.md](../../docs/research/cloud-provisioning.md)
//! §4.2): what the command writes into `settings.json`, decided and **validated
//! before anything is downloaded or saved**.
//!
//! A rented GPU box is new every time, and the last step to a managed engine
//! there used to be the settings screen — the model, the projector, the
//! embedder's model and the context, typed against a meter. The environment
//! route (`MINDFORK_MODEL` …) does not replace it: it *overrides* on every
//! launch and becomes permanent by accident the first time anything is saved
//! (research §2.2). This **writes**, so what it sets is what the settings screen
//! shows and what the user changes there afterwards sticks.
//!
//! `features` layer, pure apart from a `stat` per path: the CLI handler in
//! `main.rs` composes it with `sandbox_setup::setup`, `llama_setup::setup` and
//! the verification run (`app::verify`).

use std::path::Path;

use anyhow::{Result, bail};
use serde_json::Value;

use crate::features::cli::SetupArgs;
use crate::shared::api::managed::preflight_model;
use crate::shared::config::{AppConfig, ServerMode};
use crate::shared::i18n::Locale;

/// Top-level keys `--set` refuses outright. `schema_version` belongs to the
/// migrations (ADR 0006); `api_keys` is the machine-encrypted store (ADR 0008) —
/// a key goes in through the settings screen or stays in the environment under
/// an `api_key_env` name, never through a command line that lands in a shell
/// history.
const REFUSED_KEYS: &[&str] = &["schema_version", "api_keys"];

/// Applies the settings half of `args` to `config` and returns one printable
/// line per field it wrote. Nothing is saved here, and an `Err` leaves `config`
/// partly changed — the caller validates on its in-memory copy **before** the
/// first download, so a typo costs nothing (research §4.2).
pub fn apply_settings(
    config: &mut AppConfig,
    args: &SetupArgs,
    loc: &'static Locale,
) -> Result<Vec<String>> {
    let mut lines = Vec::new();
    let mut wrote = |key: &str, value: &str, was: Option<String>| {
        lines.push(match was {
            Some(old) => loc.tf(
                "setup.settings.line_was",
                &[("key", key), ("value", value), ("was", &old)],
            ),
            None => loc.tf("setup.settings.line", &[("key", key), ("value", value)]),
        });
    };

    if let Some(model) = &args.model {
        let path = absolute(model, loc)?;
        preflight_model(&path, loc)?;
        // Naming a model for the managed engine is asking for managed mode. A
        // data root restored from another machine may well arrive in a cloud
        // mode, and a command that then "configured" an engine nobody uses
        // would have done nothing — so the switch is made, and said.
        if config.engine.mode != ServerMode::Managed {
            let was = mode_name(config.engine.mode);
            config.engine.mode = ServerMode::Managed;
            wrote("engine.mode", "managed", Some(was));
        }
        config.engine.managed.model_path = Some(path.clone());
        wrote("engine.managed.model_path", &path, None);
    }
    if let Some(mmproj) = &args.mmproj {
        let path = absolute(mmproj, loc)?;
        if !Path::new(&path).is_file() {
            bail!(
                "{}",
                loc.tf("ui.err.managed.mmproj_not_found", &[("path", &path)])
            );
        }
        config.engine.managed.mmproj = Some(path.clone());
        wrote("engine.managed.mmproj", &path, None);
    }
    if let Some(n) = args.ctx {
        config.engine.managed.context_size = n;
        wrote("engine.managed.context_size", &n.to_string(), None);
    }
    if let Some(n) = args.ngl {
        config.engine.managed.gpu_layers = n;
        wrote("engine.managed.gpu_layers", &n.to_string(), None);
    }
    if let Some(model) = &args.embed_model {
        let path = absolute(model, loc)?;
        preflight_model(&path, loc)?;
        if config.embed.mode != ServerMode::Managed {
            let was = mode_name(config.embed.mode);
            config.embed.mode = ServerMode::Managed;
            wrote("embed.mode", "managed", Some(was));
        }
        config.embed.managed.model_path = Some(path.clone());
        wrote("embed.managed.model_path", &path, None);
    }
    // After the typed flags and in the order given, so an explicit `--set` has
    // the last word over what a flag implied.
    for (key, raw) in &args.set {
        let value = set_by_path(config, key, raw).map_err(|e| e.localized(key, raw, loc))?;
        wrote(key, &render(&value), None);
    }
    Ok(lines)
}

/// After a **successful** `--llama` install: a managed binary path that names no
/// file is cleared, so the empty field resolves to the build just installed
/// (`llama_setup::resolve_binary`). Returns the keys it cleared.
///
/// The command writes no binary path of its own — the empty field is already
/// correct and stays correct when a newer build is installed. But a data root
/// restored from another machine carries that machine's path
/// (`C:\llama\llama-server.exe` on a Linux pod), an explicit path is used
/// exactly as written, and the user asked this command for an engine that
/// works. A path that *does* name a file is a deliberate choice and is left
/// alone; so is a bare name, which the OS resolves and we cannot judge.
pub fn clear_dead_binaries(config: &mut AppConfig) -> Vec<&'static str> {
    let mut cleared = Vec::new();
    let mut visit = |key: &'static str, field: &mut Option<String>| {
        let dead = field.as_deref().map(str::trim).is_some_and(|p| {
            // Either separator, whatever the platform: the case this exists for
            // is `C:\llama\llama-server.exe` arriving on Linux, where `Path`
            // reads the whole of it as one file name with no directory part.
            let explicit = p.contains('/') || p.contains('\\');
            explicit && !Path::new(p).is_file()
        });
        if dead {
            *field = None;
            cleared.push(key);
        }
    };
    visit("engine.managed.binary", &mut config.engine.managed.binary);
    visit(
        "impersonation_engine.managed.binary",
        &mut config.impersonation_engine.managed.binary,
    );
    visit("embed.managed.binary", &mut config.embed.managed.binary);
    cleared
}

/// The path as the settings should hold it: absolute, because the TUI is not
/// launched from the directory this command was.
fn absolute(path: &Path, loc: &Locale) -> Result<String> {
    match std::path::absolute(path) {
        Ok(p) => Ok(p.display().to_string()),
        Err(e) => bail!(
            "{}",
            loc.tf(
                "setup.err.bad_path",
                &[
                    ("path", &path.display().to_string()),
                    ("detail", &e.to_string())
                ],
            )
        ),
    }
}

/// A mode, spelled as `settings.json` spells it.
fn mode_name(mode: ServerMode) -> String {
    serde_json::to_value(mode)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// A value for a report line: a string bare, anything else as JSON.
fn render(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

// -------- `--set KEY=VALUE` --------

/// Why a `--set` was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum SetError {
    /// One of [`REFUSED_KEYS`].
    Refused,
    /// No such field — or a path through something that is not an object.
    UnknownKey,
    /// The field exists and the value does not fit it; serde's own words.
    WrongType(String),
}

impl SetError {
    fn localized(&self, key: &str, raw: &str, loc: &Locale) -> anyhow::Error {
        anyhow::anyhow!(
            "{}",
            match self {
                SetError::Refused => loc.tf("setup.set.refused", &[("key", key)]),
                SetError::UnknownKey => loc.tf("setup.set.unknown_key", &[("key", key)]),
                SetError::WrongType(detail) => loc.tf(
                    "setup.set.wrong_type",
                    &[("key", key), ("value", raw), ("detail", detail)],
                ),
            }
        )
    }
}

/// Sets one field of `config` by its dotted path in `settings.json`
/// (`engine.managed.sessions`), and returns the value as it was understood.
///
/// The value is a JSON literal when it parses as one (`true`, `4`, `null`,
/// `"quoted"`, `[…]`) and a string otherwise, so a path or a mode name needs no
/// quoting. A literal that does not fit the field is tried once more **as the
/// string it was typed as** — `model_name=4` means the name "4".
///
/// The check that makes this safe to offer: `AppConfig` is `#[serde(default)]`
/// all the way down, so a key it does not know is dropped **in silence** and a
/// typo would "succeed". The document is therefore taken through the config's
/// own types and back, and what was set must still be there.
pub fn set_by_path(config: &mut AppConfig, key: &str, raw: &str) -> Result<Value, SetError> {
    let segments: Vec<&str> = key.split('.').map(str::trim).collect();
    if segments.iter().any(|s| s.is_empty()) {
        return Err(SetError::UnknownKey);
    }
    if REFUSED_KEYS.contains(&segments[0]) {
        return Err(SetError::Refused);
    }
    let literal =
        serde_json::from_str::<Value>(raw).unwrap_or_else(|_| Value::String(raw.to_string()));
    match try_set(config, &segments, &literal) {
        Ok(next) => {
            *config = next;
            Ok(literal)
        }
        Err(SetError::WrongType(first)) if !literal.is_string() => {
            let typed = Value::String(raw.to_string());
            match try_set(config, &segments, &typed) {
                Ok(next) => {
                    *config = next;
                    Ok(typed)
                }
                // The first refusal is the honest one to report: it is about
                // the value as the user most likely meant it.
                Err(_) => Err(SetError::WrongType(first)),
            }
        }
        Err(e) => Err(e),
    }
}

/// `config` with `value` at `segments`, if that is a config.
fn try_set(config: &AppConfig, segments: &[&str], value: &Value) -> Result<AppConfig, SetError> {
    let mut doc = serde_json::to_value(config).map_err(|e| SetError::WrongType(e.to_string()))?;
    let (last, parents) = segments.split_last().ok_or(SetError::UnknownKey)?;
    let mut at = &mut doc;
    for seg in parents {
        let Value::Object(map) = at else {
            return Err(SetError::UnknownKey);
        };
        // A parent left out of the serialized form (`skip_serializing_if`) is
        // created; one that does not exist at all fails the round trip below.
        at = map
            .entry((*seg).to_string())
            .or_insert_with(|| Value::Object(Default::default()));
    }
    let Value::Object(map) = at else {
        return Err(SetError::UnknownKey);
    };
    map.insert((*last).to_string(), value.clone());

    let next: AppConfig =
        serde_json::from_value(doc).map_err(|e| SetError::WrongType(e.to_string()))?;
    let back = serde_json::to_value(&next).map_err(|e| SetError::WrongType(e.to_string()))?;
    let survived = segments
        .iter()
        .try_fold(&back, |at, seg| at.as_object()?.get(*seg));
    if same(survived, value) {
        Ok(next)
    } else {
        Err(SetError::UnknownKey)
    }
}

/// Is what came back what was set? Not `==`: a number that went through an
/// `f32` field comes back as the nearest `f32`, a field left out of the
/// serialized form when empty comes back absent, and an object that named two
/// fields comes back with every field its struct has.
fn same(got: Option<&Value>, want: &Value) -> bool {
    match (got, want) {
        (None, Value::Null) => true,
        (None, Value::Array(a)) => a.is_empty(),
        (None, _) => false,
        (Some(Value::Number(a)), Value::Number(b)) => match (a.as_f64(), b.as_f64()) {
            (Some(a), Some(b)) => (a - b).abs() <= 1e-6 * a.abs().max(b.abs()).max(1.0),
            _ => a == b,
        },
        (Some(Value::Object(a)), Value::Object(b)) => b.iter().all(|(k, v)| same(a.get(k), v)),
        (Some(Value::Array(a)), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same(Some(x), y))
        }
        (Some(a), b) => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::i18n::{Lang, locale};
    use std::path::PathBuf;

    fn gguf(dir: &Path, name: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, b"GGUF").unwrap();
        p
    }

    /// The whole sketch in one call: the model, the projector, the embedder's
    /// model, the context and the layers — and the two modes that naming a
    /// model implies, which are said only when they actually changed.
    #[test]
    fn the_five_named_settings_land_and_a_changed_mode_is_said() {
        let loc = locale(Lang::En);
        let dir = tempfile::tempdir().unwrap();
        let model = gguf(dir.path(), "chat.gguf");
        let mmproj = gguf(dir.path(), "mmproj.gguf");
        let embed = gguf(dir.path(), "embed.gguf");
        let mut config = AppConfig::default();
        config.engine.mode = ServerMode::Claude; // restored from a machine that used a cloud
        let args = SetupArgs {
            model: Some(model.clone()),
            mmproj: Some(mmproj.clone()),
            embed_model: Some(embed.clone()),
            ctx: Some(32768),
            ngl: Some(40),
            ..Default::default()
        };
        let lines = apply_settings(&mut config, &args, loc).unwrap();

        assert_eq!(config.engine.mode, ServerMode::Managed);
        assert_eq!(config.embed.mode, ServerMode::Managed);
        let m = &config.engine.managed;
        assert_eq!(m.model_path.as_deref(), Some(model.to_str().unwrap()));
        assert_eq!(m.mmproj.as_deref(), Some(mmproj.to_str().unwrap()));
        assert_eq!((m.context_size, m.gpu_layers), (32768, 40));
        assert_eq!(
            config.embed.managed.model_path.as_deref(),
            Some(embed.to_str().unwrap())
        );
        // No binary path is written: the empty field resolves by itself.
        assert_eq!(m.binary, None);

        let text = lines.join("\n");
        assert!(text.contains("engine.mode = managed"), "{text}");
        assert!(text.contains("claude"), "the old mode is named: {text}");
        // The embedder was managed by default — nothing changed, nothing said.
        assert!(!text.contains("embed.mode"), "{text}");
        assert!(
            text.contains("engine.managed.context_size = 32768"),
            "{text}"
        );
    }

    /// A step that was not named changes nothing: `--ctx` alone is a use.
    #[test]
    fn a_field_that_was_not_named_is_left_alone() {
        let loc = locale(Lang::En);
        let mut config = AppConfig::default();
        config.engine.mode = ServerMode::External;
        config.engine.managed.model_path = Some("/keep/me.gguf".into());
        let before = config.clone();
        let args = SetupArgs {
            ctx: Some(65536),
            ..Default::default()
        };
        let lines = apply_settings(&mut config, &args, loc).unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(config.engine.managed.context_size, 65536);
        config.engine.managed.context_size = before.engine.managed.context_size;
        assert_eq!(config, before, "only the context moved — not even the mode");
    }

    /// A relative path is stored absolute: the TUI starts somewhere else.
    #[test]
    fn a_relative_path_is_stored_absolute() {
        let loc = locale(Lang::En);
        let dir = tempfile::tempdir().unwrap();
        let model = gguf(dir.path(), "rel.gguf");
        // Relative to the current directory, whatever it is: strip a prefix if
        // there is one, otherwise the absolute path already proves the point.
        let cwd = std::env::current_dir().unwrap();
        let given = model
            .strip_prefix(&cwd)
            .map(Path::to_path_buf)
            .unwrap_or(model.clone());
        let mut config = AppConfig::default();
        let args = SetupArgs {
            model: Some(given),
            ..Default::default()
        };
        apply_settings(&mut config, &args, loc).unwrap();
        let stored = config.engine.managed.model_path.unwrap();
        assert!(Path::new(&stored).is_absolute(), "{stored}");
    }

    /// A typo is an error **now**, with the launch's own words — not a status
    /// line in a TUI ten minutes and a gigabyte later. Both locales: these are
    /// the refusals a person on a meter reads.
    #[test]
    fn a_path_that_names_no_file_is_refused_in_the_launchs_own_words() {
        let dir = tempfile::tempdir().unwrap();
        let real = gguf(dir.path(), "real.gguf");
        for lang in [Lang::En, Lang::Ru] {
            let loc = locale(lang);
            for args in [
                SetupArgs {
                    model: Some(dir.path().join("nope.gguf")),
                    ..Default::default()
                },
                SetupArgs {
                    embed_model: Some(dir.path().join("nope.gguf")),
                    ..Default::default()
                },
            ] {
                let err = apply_settings(&mut AppConfig::default(), &args, loc).unwrap_err();
                let path = dir.path().join("nope.gguf").display().to_string();
                assert_eq!(
                    err.to_string(),
                    loc.tf("ui.err.managed.model_not_found", &[("path", &path)]),
                    "{lang:?}"
                );
            }
            let args = SetupArgs {
                model: Some(real.clone()),
                mmproj: Some(dir.path().join("no-projector.gguf")),
                ..Default::default()
            };
            let err = apply_settings(&mut AppConfig::default(), &args, loc).unwrap_err();
            assert!(err.to_string().contains("no-projector.gguf"), "{err}");
        }
    }

    /// A part of a multi-file GGUF that is not the first is refused here too —
    /// the same preflight the launch runs, so the two cannot disagree.
    #[test]
    fn a_split_model_must_be_named_by_its_first_part() {
        let loc = locale(Lang::En);
        let dir = tempfile::tempdir().unwrap();
        gguf(dir.path(), "big-00001-of-00002.gguf");
        let second = gguf(dir.path(), "big-00002-of-00002.gguf");
        let args = SetupArgs {
            model: Some(second),
            ..Default::default()
        };
        let err = apply_settings(&mut AppConfig::default(), &args, loc).unwrap_err();
        assert!(err.to_string().contains("00001-of-00002"), "{err}");
    }

    #[test]
    fn set_reaches_any_field_by_its_path_in_the_file() {
        let mut c = AppConfig::default();
        assert_eq!(
            set_by_path(&mut c, "engine.managed.sessions", "4"),
            Ok(Value::from(4))
        );
        assert_eq!(c.engine.managed.sessions, 4);
        assert_eq!(
            set_by_path(&mut c, "engine.managed.no_mmap", "true"),
            Ok(Value::Bool(true))
        );
        assert!(c.engine.managed.no_mmap);
        // A bare word is a string: a mode, a path, a host need no quoting.
        set_by_path(&mut c, "engine.managed.host", "0.0.0.0").unwrap();
        assert_eq!(c.engine.managed.host, "0.0.0.0");
        set_by_path(&mut c, "engine.mode", "external").unwrap();
        assert_eq!(c.engine.mode, ServerMode::External);
        set_by_path(&mut c, "engine.managed.flash_attn", "on").unwrap();
        set_by_path(&mut c, "tools.python_enabled", "true").unwrap();
        assert!(c.tools.python_enabled);
        // `null` unsets an optional field.
        c.engine.managed.mmproj = Some("/x".into());
        set_by_path(&mut c, "engine.managed.mmproj", "null").unwrap();
        assert_eq!(c.engine.managed.mmproj, None);
        // A number through an `f32` comes back as the nearest `f32`; that is
        // still the value that was set.
        set_by_path(&mut c, "default_sampling.temperature", "0.7").unwrap();
        assert_eq!(c.default_sampling.temperature, Some(0.7));
    }

    /// A literal that does not fit is tried as the text it was typed as: a model
    /// called "4" is a name, and a port typed as a word is still an error.
    #[test]
    fn a_literal_falls_back_to_the_string_it_was_typed_as() {
        let mut c = AppConfig::default();
        assert_eq!(
            set_by_path(&mut c, "engine.external.model_name", "4"),
            Ok(Value::String("4".into()))
        );
        assert_eq!(c.engine.external.model_name.as_deref(), Some("4"));
        let err = set_by_path(&mut c, "engine.managed.port", "eight-thousand").unwrap_err();
        assert!(matches!(err, SetError::WrongType(_)), "{err:?}");
        // And the reported refusal is about the value as it was meant.
        let err = set_by_path(&mut c, "engine.managed.port", "true").unwrap_err();
        let SetError::WrongType(detail) = err else {
            panic!("{err:?}")
        };
        assert!(detail.contains("boolean"), "{detail}");
    }

    /// The reason the round trip exists: every struct here is
    /// `#[serde(default)]`, so an unknown key deserializes away in silence.
    #[test]
    fn a_key_the_config_does_not_have_is_refused_not_dropped() {
        let mut c = AppConfig::default();
        let before = c.clone();
        for key in [
            "engine.managed.modle_path", // the typo this exists for
            "engine.manged.model_path",
            "nonsense",
            "engine.managed.port.deeper",
            "engine..mode",
            "",
        ] {
            assert_eq!(
                set_by_path(&mut c, key, "x"),
                Err(SetError::UnknownKey),
                "{key:?}"
            );
        }
        assert_eq!(c, before, "a refused key changes nothing");
    }

    #[test]
    fn the_migrations_version_and_the_key_store_are_not_settable() {
        let mut c = AppConfig::default();
        let before = c.clone();
        assert_eq!(
            set_by_path(&mut c, "schema_version", "1"),
            Err(SetError::Refused)
        );
        assert_eq!(
            set_by_path(&mut c, "api_keys", "[]"),
            Err(SetError::Refused)
        );
        assert_eq!(c, before);
    }

    /// `--set` comes after the typed flags and has the last word, and every
    /// refusal is localized in both languages with the key in it.
    #[test]
    fn set_runs_after_the_flags_and_its_refusals_name_the_key() {
        let dir = tempfile::tempdir().unwrap();
        let model = gguf(dir.path(), "m.gguf");
        let loc = locale(Lang::En);
        let mut config = AppConfig::default();
        let args = SetupArgs {
            model: Some(model),
            ctx: Some(4096),
            set: vec![
                ("engine.managed.context_size".into(), "8192".into()),
                ("engine.managed.sessions".into(), "2".into()),
            ],
            ..Default::default()
        };
        let lines = apply_settings(&mut config, &args, loc).unwrap();
        assert_eq!(config.engine.managed.context_size, 8192);
        assert!(
            lines
                .last()
                .unwrap()
                .contains("engine.managed.sessions = 2")
        );

        for lang in [Lang::En, Lang::Ru] {
            let loc = locale(lang);
            for (key, value) in [
                ("engine.managed.modle_path", "x"),
                ("api_keys", "[]"),
                ("engine.managed.port", "nope"),
            ] {
                let args = SetupArgs {
                    set: vec![(key.into(), value.into())],
                    ..Default::default()
                };
                let err = apply_settings(&mut AppConfig::default(), &args, loc).unwrap_err();
                assert!(err.to_string().contains(key), "{lang:?} {key}: {err}");
                assert!(!err.to_string().contains('{'), "{lang:?} {key}: {err}");
            }
        }
    }

    /// A path from another machine is cleared so the fresh install is found; a
    /// path that names a file, and a bare name the OS resolves, are the user's.
    #[test]
    fn only_a_binary_path_that_names_no_file_is_cleared() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("llama-server");
        std::fs::write(&real, b"x").unwrap();
        let mut c = AppConfig::default();
        // Spelled the Windows way on purpose: on Linux `Path` sees no directory
        // part in it at all, and it is exactly the path a restored data root
        // brings to a pod.
        c.engine.managed.binary = Some(r"C:\llama\llama-server.exe".into());
        c.impersonation_engine.managed.binary = Some(real.display().to_string());
        c.embed.managed.binary = Some("llama-server".into());
        assert_eq!(clear_dead_binaries(&mut c), ["engine.managed.binary"]);
        assert_eq!(c.engine.managed.binary, None);
        c.engine.managed.binary = Some("/opt/gone/llama-server".into());
        assert_eq!(clear_dead_binaries(&mut c), ["engine.managed.binary"]);
        assert_eq!(
            c.impersonation_engine.managed.binary.as_deref(),
            Some(real.to_str().unwrap())
        );
        assert_eq!(c.embed.managed.binary.as_deref(), Some("llama-server"));
        // Nothing set: nothing to clear, and nothing reported.
        assert!(clear_dead_binaries(&mut AppConfig::default()).is_empty());
    }
}
