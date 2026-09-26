//! The raw arguments a user adds to a managed `llama-server` line (spec §3.4,
//! docs/research/managed-extra-args.md): which of them the app refuses, and which
//! `LLAMA_*` environment variables it keeps away from the child.
//!
//! The field is a free-form argv appended **last** to the line `build_args`
//! writes. Measured (§3 there): a repeated flag silently wins — `--port` moves
//! the server away from the port the app probes — and upstream has deprecated the
//! repetition itself, so an override that works today may stop a launch
//! tomorrow. Four kinds of flag are refused (§4, forks F3/F4):
//!
//! 1. one the section already has a **field** for — the message names it, so a
//!    knob has one place and the screen never shows a value that is not running;
//! 2. one the app's **connection** to its own child depends on (a key, a path
//!    prefix, TLS) — the server would come up and refuse the app;
//! 3. one that makes the server an **agent** with the host's hands (`--tools`,
//!    `--agent`, MCP) — a shell for whoever reaches the port;
//! 4. one that makes it **fetch** or route models by itself — what managed mode
//!    refuses router mode for (`ManagedConfig::is_runnable`).
//!
//! Kinds 2–4 name the door that works: start `llama-server` yourself and connect
//! in External mode. Their environment variables are removed from the child's
//! environment (F7), so a refused flag cannot come in by the side door — and a
//! `LLAMA_API_KEY` set for some other tool no longer locks the app out of a server
//! it reports ready (measured, M9).
//!
//! **The spellings are llama.cpp's and they move** — build 11191 (`4b1a27fa0`,
//! 2026-09-26) is what this table was read from, plus the `--no-mmap`/`--mmap`
//! pair older builds still take (`NoMmapSpelling`). A spelling the table misses
//! is not refused: kind 1 then falls back to "the last occurrence wins", which is
//! upstream's own behaviour. The `#[ignore]` smoke `the_table_matches_the_binary_live`
//! reads the real binary's `--help` so a rename is caught by a run.

use crate::shared::i18n::Locale;

/// Which managed server a line is for — the three sections show different fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ManagedRole {
    /// The assistant's chat server ("Model" → "Assistant").
    #[default]
    Assistant,
    /// The impersonation server — the same fields, less "Sessions".
    Impersonation,
    /// The embedding server — a model and a port; the app pins the rest.
    Embedder,
}

/// A settings field that writes a flag of its own (kind 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnField {
    Host,
    Port,
    Model,
    Mmproj,
    Context,
    GpuLayers,
    Batch,
    Sessions,
    Jinja,
    FlashAttn,
    NoMmap,
    SpecType,
    DraftModel,
    DraftGpuLayers,
    DraftNMax,
    DraftNMin,
}

impl OwnField {
    /// Every field, for the tests that pin each label to a row on the screen.
    #[cfg(test)]
    pub const ALL: [OwnField; 16] = [
        OwnField::Host,
        OwnField::Port,
        OwnField::Model,
        OwnField::Mmproj,
        OwnField::Context,
        OwnField::GpuLayers,
        OwnField::Batch,
        OwnField::Sessions,
        OwnField::Jinja,
        OwnField::FlashAttn,
        OwnField::NoMmap,
        OwnField::SpecType,
        OwnField::DraftModel,
        OwnField::DraftGpuLayers,
        OwnField::DraftNMax,
        OwnField::DraftNMin,
    ];

    /// Whether `role`'s section shows this field — a flag is refused as "set by
    /// a field" only where there is one to set it by.
    pub fn shown_in(self, role: ManagedRole) -> bool {
        match self {
            OwnField::Port | OwnField::Model => true,
            OwnField::Sessions => role == ManagedRole::Assistant,
            _ => role != ManagedRole::Embedder,
        }
    }

    /// The field's label as the settings screen shows it. The draft fields
    /// appear only once the speculative type needs a draft model, so their
    /// label is the path to them. `screens/settings` pins each against the
    /// row it names.
    pub fn label(self, loc: &Locale) -> String {
        let key = match self {
            // These three are literal labels on the screen, not bundle keys.
            OwnField::Host => return "Host".to_string(),
            OwnField::FlashAttn => return "FlashAttn (--flash-attn)".to_string(),
            OwnField::NoMmap => return "No mmap".to_string(),
            OwnField::Port => "ui.settings.field.port",
            OwnField::Model => "ui.settings.field.gguf",
            OwnField::Mmproj => "ui.settings.field.mmproj",
            OwnField::Context => "ui.settings.field.context",
            OwnField::GpuLayers => "ui.settings.field.ngl",
            OwnField::Batch => "ui.settings.field.batch",
            OwnField::Sessions => "ui.settings.field.sessions",
            OwnField::Jinja => "ui.settings.field.jinja",
            OwnField::SpecType => "ui.settings.field.spec_type",
            OwnField::DraftModel => "ui.settings.field.draft_model",
            OwnField::DraftGpuLayers => "ui.settings.field.draft_ngl",
            OwnField::DraftNMax => "ui.settings.field.draft_n_max",
            OwnField::DraftNMin => "ui.settings.field.draft_n_min",
        };
        match self {
            OwnField::DraftModel
            | OwnField::DraftGpuLayers
            | OwnField::DraftNMax
            | OwnField::DraftNMin => {
                format!("{} → {}", loc.t("ui.settings.field.spec_type"), loc.t(key))
            }
            _ => loc.t(key).to_string(),
        }
    }
}

/// Why an extra argument is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Kind 1: the section's own field writes it.
    Field(OwnField),
    /// Kind 2: the app would lose the server it started.
    Connection,
    /// Kind 3: the server would run commands and touch files for its callers.
    Agent,
    /// Kind 4: the server would download or route models by itself.
    Fetch,
}

/// The first refused argument of a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    /// The flag as the user typed it (a `--flag=value` keeps its value).
    pub flag: String,
    pub why: Refusal,
}

impl Refused {
    /// One line for the field editor's title and the server's status: the flag,
    /// what it would do, and where to go instead.
    pub fn message(&self, loc: &Locale) -> String {
        let flag = ("flag", self.flag.as_str());
        match self.why {
            Refusal::Field(field) => loc.tf(
                "ui.err.managed.extra_arg.field",
                &[flag, ("field", &field.label(loc))],
            ),
            Refusal::Connection => loc.tf("ui.err.managed.extra_arg.connection", &[flag]),
            Refusal::Agent => loc.tf("ui.err.managed.extra_arg.agent", &[flag]),
            Refusal::Fetch => loc.tf("ui.err.managed.extra_arg.fetch", &[flag]),
        }
    }

    /// The launch's refusal, for the server's status: the setting it came from,
    /// and — where no field of the section can do what the flag does — the way
    /// that can (kinds 2–4: the user's own `llama-server`, reached in External
    /// mode).
    pub fn launch_message(&self, loc: &Locale) -> String {
        let mut reason = self.message(loc);
        if !matches!(self.why, Refusal::Field(_)) {
            reason = format!("{reason} — {}", loc.t("ui.err.managed.extra_arg.door"));
        }
        loc.tf("ui.err.managed.extra_args", &[("reason", &reason)])
    }
}

/// How a row of the table is judged; a function of the role for the two flags
/// whose meaning differs between the chat servers and the embedder.
#[derive(Debug, Clone, Copy)]
enum Rule {
    Field(OwnField),
    /// A field on the chat servers; on the embedder, which the app binds to
    /// loopback itself, the connection.
    Host,
    /// The embedder's own flag; on a chat server it leaves nothing to chat with.
    Embeddings,
    Connection,
    Agent,
    Fetch,
}

impl Rule {
    fn verdict(self, role: ManagedRole) -> Option<Refusal> {
        match self {
            Rule::Field(f) => f.shown_in(role).then_some(Refusal::Field(f)),
            Rule::Host if role == ManagedRole::Embedder => Some(Refusal::Connection),
            Rule::Host => Some(Refusal::Field(OwnField::Host)),
            Rule::Embeddings => (role != ManagedRole::Embedder).then_some(Refusal::Connection),
            Rule::Connection => Some(Refusal::Connection),
            Rule::Agent => Some(Refusal::Agent),
            Rule::Fetch => Some(Refusal::Fetch),
        }
    }
}

/// One llama.cpp option: every spelling it answers to, the environment
/// variables that set it, and the rule. Only the positive spellings of kinds
/// 3–4 are listed — `--no-agent` is what the app wants anyway — while a field's
/// negative spelling (`--no-jinja`) is as much a conflict as its positive one.
struct Row {
    spellings: Vec<&'static str>,
    env: Vec<&'static str>,
    rule: Rule,
}

/// Kind 1, read from build 11191's `--help` (see the module doc): the options a
/// field of the section writes. No environment names — a field's flag is always
/// on the app's line, and the command line beats the environment (M8).
const FIELD_ROWS: &[(&[&str], Rule)] = &[
    (&["--host"], Rule::Host),
    (&["--port"], Rule::Field(OwnField::Port)),
    (&["-m", "--model"], Rule::Field(OwnField::Model)),
    (&["-mm", "--mmproj"], Rule::Field(OwnField::Mmproj)),
    (&["-c", "--ctx-size"], Rule::Field(OwnField::Context)),
    (
        &["-ngl", "--gpu-layers", "--n-gpu-layers"],
        Rule::Field(OwnField::GpuLayers),
    ),
    (&["-b", "--batch-size"], Rule::Field(OwnField::Batch)),
    (&["-np", "--parallel"], Rule::Field(OwnField::Sessions)),
    (&["--jinja", "--no-jinja"], Rule::Field(OwnField::Jinja)),
    (&["-fa", "--flash-attn"], Rule::Field(OwnField::FlashAttn)),
    (
        &["-lm", "--load-mode", "--no-mmap", "--mmap"],
        Rule::Field(OwnField::NoMmap),
    ),
    (&["--spec-type"], Rule::Field(OwnField::SpecType)),
    (
        &["-md", "--model-draft", "--spec-draft-model"],
        Rule::Field(OwnField::DraftModel),
    ),
    (
        &[
            "-ngld",
            "--gpu-layers-draft",
            "--n-gpu-layers-draft",
            "--spec-draft-ngl",
        ],
        Rule::Field(OwnField::DraftGpuLayers),
    ),
    (
        &["--spec-draft-n-max", "--draft", "--draft-n", "--draft-max"],
        Rule::Field(OwnField::DraftNMax),
    ),
    (
        &["--spec-draft-n-min", "--draft-min", "--draft-n-min"],
        Rule::Field(OwnField::DraftNMin),
    ),
];

/// Kinds 2–4, from the same `--help`, one option a line: the kind, the
/// spellings, and after `env:` the variables the child's environment loses. A
/// text table because the rows differ only in their strings — as code they would
/// be twenty copies of one shape (docs/lessons.md §2) — and because it reads as
/// the list it is. `embeddings` is kind 2 on a chat server and the embedder's own
/// flag (`Rule::Embeddings`).
const REFUSED_ROWS: &str = "
    embeddings  --embedding --embeddings                     env: LLAMA_ARG_EMBEDDINGS
    connection  --api-key                                    env: LLAMA_API_KEY
    connection  --api-key-file                               env: LLAMA_ARG_API_KEY_FILE
    connection  --api-prefix                                 env: LLAMA_ARG_API_PREFIX
    connection  --ssl-key-file                               env: LLAMA_ARG_SSL_KEY_FILE
    connection  --ssl-cert-file                              env: LLAMA_ARG_SSL_CERT_FILE
    agent       --tools                                      env: LLAMA_ARG_TOOLS
    agent       --tools-runtime                              env: LLAMA_ARG_TOOLS_RUNTIME
    agent       -ag --agent                                  env: LLAMA_ARG_AGENT
    agent       --mcp-servers-config                         env: LLAMA_ARG_MCP_SERVERS_CONFIG
    agent       --mcp-servers-json                           env: LLAMA_ARG_MCP_SERVERS_JSON
    agent       --ui-mcp-proxy --webui-mcp-proxy             env: LLAMA_ARG_UI_MCP_PROXY
    fetch       -hf -hfr --hf-repo                           env: LLAMA_ARG_HF_REPO
    fetch       -hff --hf-file                               env: LLAMA_ARG_HF_FILE
    fetch       -hft --hf-token                              env: HF_TOKEN
    fetch       --spec-draft-hf -hfd -hfrd --hf-repo-draft   env: LLAMA_ARG_SPEC_DRAFT_HF_REPO
    fetch       -mu --model-url                              env: LLAMA_ARG_MODEL_URL
    fetch       -mmu --mmproj-url                            env: LLAMA_ARG_MMPROJ_URL
    fetch       -dr --docker-repo                            env: LLAMA_ARG_DOCKER_REPO
    fetch       --models-dir                                 env: LLAMA_ARG_MODELS_DIR
    fetch       --models-preset                              env: LLAMA_ARG_MODELS_PRESET
";
// `HF_TOKEN` rides with `-hft`: useful only beside a download, and a secret a
// server that downloads nothing has no business holding.

/// Both halves as one list, built on first use.
fn table() -> &'static [Row] {
    static TABLE: std::sync::LazyLock<Vec<Row>> = std::sync::LazyLock::new(|| {
        let fields = FIELD_ROWS.iter().map(|(spellings, rule)| Row {
            spellings: spellings.to_vec(),
            env: Vec::new(),
            rule: *rule,
        });
        let refused = REFUSED_ROWS
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(refused_row);
        fields.chain(refused).collect()
    });
    &TABLE
}

/// One line of [`REFUSED_ROWS`]. A malformed line is a mistake in the constant
/// above, which every test of this module reads.
fn refused_row(line: &'static str) -> Row {
    let (names, env) = line
        .split_once("env:")
        .expect("a refused row names its variables");
    let mut words = names.split_whitespace();
    let rule = match words.next() {
        Some("embeddings") => Rule::Embeddings,
        Some("connection") => Rule::Connection,
        Some("agent") => Rule::Agent,
        Some("fetch") => Rule::Fetch,
        other => panic!("REFUSED_ROWS: unknown kind {other:?}"),
    };
    Row {
        spellings: words.collect(),
        env: env.split_whitespace().collect(),
        rule,
    }
}

/// The option name a token spells, if it spells one: `--flag=value` is not a
/// form llama.cpp takes (M3), but it is one a person types, and the flag in it
/// is what the answer is about.
fn option_name(token: &str) -> &str {
    if token.starts_with('-') {
        token.split_once('=').map_or(token, |(name, _)| name)
    } else {
        token
    }
}

/// The first argument of `args` that `role`'s line refuses, or `Ok`.
///
/// Every token is compared, values included: no value llama.cpp takes is
/// spelled like one of these flags, and knowing each option's arity would mean
/// carrying the whole option table rather than these rows.
pub fn check(args: &[String], role: ManagedRole) -> Result<(), Refused> {
    for token in args {
        let name = option_name(token);
        let refusal = table()
            .iter()
            .find(|r| r.spellings.contains(&name))
            .and_then(|r| r.rule.verdict(role));
        if let Some(why) = refusal {
            return Err(Refused {
                flag: token.clone(),
                why,
            });
        }
    }
    Ok(())
}

/// The environment variables removed from `role`'s child (F7): those of every
/// flag the role refuses for kinds 2–4.
pub fn scrubbed_env(role: ManagedRole) -> impl Iterator<Item = &'static str> {
    table()
        .iter()
        .filter(move |r| {
            matches!(
                r.rule.verdict(role),
                Some(Refusal::Connection | Refusal::Agent | Refusal::Fetch)
            )
        })
        .flat_map(|r| r.env.iter().copied())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::i18n::{Lang, locale};

    fn argv(line: &str) -> Vec<String> {
        crate::shared::cmdline::split(line)
    }

    fn refused(line: &str, role: ManagedRole) -> Option<Refusal> {
        check(&argv(line), role).err().map(|r| r.why)
    }

    /// The use the field exists for passes on every server.
    #[test]
    fn the_flags_nothing_else_writes_pass() {
        for role in [
            ManagedRole::Assistant,
            ManagedRole::Impersonation,
            ManagedRole::Embedder,
        ] {
            assert_eq!(
                check(
                    &argv(r#"--n-cpu-moe 20 -ot "blk\.(1[0-9])\.ffn_.*_exps\.=CPU" -t 8 -ub 1024"#),
                    role
                ),
                Ok(()),
                "{role:?}"
            );
        }
    }

    /// Kind 1 answers to every spelling of the option, not just the one the
    /// app writes: `--ctx-size` repeats the app's `-c` exactly as `-c` would.
    #[test]
    fn a_field_flag_is_refused_in_any_spelling() {
        let a = ManagedRole::Assistant;
        for line in ["-c 8192", "--ctx-size 8192", "--ctx-size=8192"] {
            assert_eq!(
                refused(line, a),
                Some(Refusal::Field(OwnField::Context)),
                "{line}"
            );
        }
        assert_eq!(
            refused("--n-gpu-layers 30", a),
            Some(Refusal::Field(OwnField::GpuLayers))
        );
        assert_eq!(
            refused("--no-jinja", a),
            Some(Refusal::Field(OwnField::Jinja))
        );
        assert_eq!(
            refused("--port 9000", a),
            Some(Refusal::Field(OwnField::Port))
        );
        // And the refusal carries the flag as typed.
        let r = check(&argv("-t 8 --ctx-size=8192"), a).unwrap_err();
        assert_eq!(r.flag, "--ctx-size=8192");
    }

    /// A flag is "set by a field" only where the section has that field: the
    /// impersonation section has no Sessions, and the embedder's shows only a
    /// model and a port — so its context and GPU layers are this field's to set.
    #[test]
    fn a_field_flag_is_refused_only_where_the_field_is() {
        assert_eq!(
            refused("-np 4", ManagedRole::Assistant),
            Some(Refusal::Field(OwnField::Sessions))
        );
        assert_eq!(refused("-np 4", ManagedRole::Impersonation), None);
        for line in ["-c 4096", "-ngl 0", "-b 512", "--flash-attn on"] {
            assert_eq!(refused(line, ManagedRole::Embedder), None, "{line}");
        }
        for (line, field) in [("-m x.gguf", OwnField::Model), ("--port 1", OwnField::Port)] {
            assert_eq!(
                refused(line, ManagedRole::Embedder),
                Some(Refusal::Field(field)),
                "{line}"
            );
        }
    }

    /// The two flags whose meaning depends on the server.
    #[test]
    fn host_and_embeddings_are_judged_per_server() {
        assert_eq!(
            refused("--host 0.0.0.0", ManagedRole::Assistant),
            Some(Refusal::Field(OwnField::Host))
        );
        assert_eq!(
            refused("--host 0.0.0.0", ManagedRole::Embedder),
            Some(Refusal::Connection)
        );
        assert_eq!(
            refused("--embeddings", ManagedRole::Impersonation),
            Some(Refusal::Connection)
        );
        assert_eq!(refused("--embeddings", ManagedRole::Embedder), None);
    }

    #[test]
    fn kinds_two_to_four_are_refused_everywhere() {
        for role in [
            ManagedRole::Assistant,
            ManagedRole::Impersonation,
            ManagedRole::Embedder,
        ] {
            assert_eq!(refused("--api-key s3cret", role), Some(Refusal::Connection));
            assert_eq!(refused("--api-prefix /x", role), Some(Refusal::Connection));
            assert_eq!(refused("--tools all", role), Some(Refusal::Agent));
            assert_eq!(refused("--tools=all", role), Some(Refusal::Agent));
            assert_eq!(refused("-ag", role), Some(Refusal::Agent));
            assert_eq!(refused("-hf ggml-org/x", role), Some(Refusal::Fetch));
            assert_eq!(refused("--models-dir d", role), Some(Refusal::Fetch));
        }
    }

    /// The negations of kind 3 are what the app wants, and look-alike names
    /// that are other options entirely (`--no-host` is a buffer setting) pass.
    #[test]
    fn negations_and_look_alikes_pass() {
        let a = ManagedRole::Assistant;
        for line in [
            "--no-agent",
            "-no-ag",
            "--no-ui-mcp-proxy",
            "--no-host",
            "--rerank",
        ] {
            assert_eq!(refused(line, a), None, "{line}");
        }
    }

    /// Every flag the app's own line can carry is either refused as a field's,
    /// or one the plan (F3) deliberately leaves open: `-ub` (the common GPU
    /// tuning, written only beside `-b`), the unified KV pool, the reasoning
    /// format (no row on the screen), and the embedder's pair. A new flag in
    /// `build_args` fails this until it is judged.
    #[test]
    fn every_flag_the_app_writes_is_judged() {
        use crate::shared::api::managed::{ManagedConfig, NoMmapSpelling, build_args};
        let cfg = ManagedConfig {
            binary: "llama-server".into(),
            model_path: Some("m.gguf".into()),
            mmproj: Some("p.gguf".into()),
            gpu_layers: 0,
            context_size: 4096,
            batch_size: None,
            parallel: 2,
            jinja: true,
            reasoning_format: Some("auto".into()),
            embeddings: false,
            no_mmap: true,
            flash_attn: Some("on".into()),
            spec_type: Some("draft-simple".into()),
            draft_model: Some("d.gguf".into()),
            draft_gpu_layers: Some(1),
            draft_n_max: Some(3),
            draft_n_min: Some(0),
            host: "127.0.0.1".into(),
            port: 8000,
            role: ManagedRole::Assistant,
            extra_args: vec![],
        };
        let open = ["-ub", "--kv-unified", "--reasoning-format", "--embeddings"];
        for spelling in [NoMmapSpelling::LoadMode, NoMmapSpelling::NoMmap] {
            for flag in build_args(&cfg, spelling)
                .iter()
                .filter(|a| a.starts_with('-') && a.parse::<f64>().is_err())
            {
                if open.contains(&flag.as_str()) {
                    continue;
                }
                assert!(
                    matches!(
                        refused(flag, ManagedRole::Assistant),
                        Some(Refusal::Field(_))
                    ),
                    "{flag} is on the app's line and not refused as a field's"
                );
            }
        }
    }

    #[test]
    fn the_environment_of_every_refused_flag_is_scrubbed() {
        let chat: Vec<_> = scrubbed_env(ManagedRole::Assistant).collect();
        for var in [
            "LLAMA_API_KEY",
            "LLAMA_ARG_TOOLS",
            "LLAMA_ARG_HF_REPO",
            "LLAMA_ARG_EMBEDDINGS",
        ] {
            assert!(chat.contains(&var), "{var} missing from {chat:?}");
        }
        // A field's flag is on the command line, which beats the environment:
        // nothing to remove, and a user's LLAMA_ARG_* for an open flag stays.
        assert!(!chat.contains(&"LLAMA_ARG_CTX_SIZE"));
        // The embedder is started with `--embeddings`; its variable is harmless there.
        let embed: Vec<_> = scrubbed_env(ManagedRole::Embedder).collect();
        assert!(!embed.contains(&"LLAMA_ARG_EMBEDDINGS"));
        assert!(embed.contains(&"LLAMA_API_KEY"));
    }

    /// Every message names the flag; a field's names the field; the rest name
    /// the door that works.
    #[test]
    fn messages_name_the_flag_and_the_way_out() {
        let en = locale(Lang::En);
        let r = check(&argv("--ctx-size 8192"), ManagedRole::Assistant).unwrap_err();
        let m = r.message(en);
        assert!(
            m.contains("--ctx-size") && m.contains("Context (-c)"),
            "{m}"
        );
        let r = check(&argv("-ngld 99"), ManagedRole::Assistant).unwrap_err();
        let m = r.message(en);
        assert!(m.contains("→ Draft GPU layers (-ngld)"), "{m}");
        for line in ["--api-key k", "--tools all", "-hf a/b"] {
            let r = check(&argv(line), ManagedRole::Assistant).unwrap_err();
            assert!(r.message(en).contains(argv(line)[0].as_str()), "{line}");
            let m = r.launch_message(en);
            assert!(
                m.contains("Extra arguments") && m.contains("External"),
                "{line}: {m}"
            );
        }
        // A field's refusal already says where to go; the launch adds no door.
        let r = check(&argv("-c 1"), ManagedRole::Assistant).unwrap_err();
        assert!(!r.launch_message(en).contains("External"));
        for lang in [Lang::En, Lang::Ru] {
            let m = check(&argv("--tools all"), ManagedRole::Assistant)
                .unwrap_err()
                .message(locale(lang));
            assert!(!m.contains('{'), "an argument left unsubstituted: {m}");
        }
    }

    /// The table against the binary it describes: every spelling the table
    /// carries is one `--help` still lists (bar the pre-2026-07 pair the launcher
    /// itself still speaks), every other spelling `--help` lists for the same
    /// option is in the row, and a refused option's environment names are all
    /// scrubbed. A rename upstream fails this run instead of quietly turning a
    /// refusal into "the last occurrence wins" (docs/lessons.md §3: a measured
    /// fact about someone else's release has a date on it).
    ///
    ///     MINDFORK_LLAMA_BIN=.../llama-server.exe \
    ///       cargo test the_table_matches_the_binary -- --ignored --nocapture
    #[test]
    #[ignore = "requires a local llama-server binary (MINDFORK_LLAMA_BIN)"]
    fn the_table_matches_the_binary_live() {
        let Ok(bin) = std::env::var("MINDFORK_LLAMA_BIN") else {
            eprintln!("skip: MINDFORK_LLAMA_BIN not set");
            return;
        };
        let out = std::process::Command::new(&bin)
            .arg("--help")
            .output()
            .expect("run --help");
        let help = String::from_utf8_lossy(&out.stdout).into_owned()
            + &String::from_utf8_lossy(&out.stderr);
        let options = help_options(&help);
        assert!(
            options.len() > 100,
            "--help parsed to {} options — not a llama-server help text",
            options.len()
        );
        const LEGACY: [&str; 2] = ["--no-mmap", "--mmap"];
        for row in table() {
            let listed: Vec<&(Vec<String>, Vec<String>)> = options
                .iter()
                .filter(|(names, _)| row.spellings.iter().any(|s| names.iter().any(|n| n == s)))
                .collect();
            for spelling in &row.spellings {
                assert!(
                    LEGACY.contains(spelling)
                        || listed.iter().any(|(n, _)| n.iter().any(|x| x == spelling)),
                    "{spelling} is no longer in --help"
                );
            }
            for (names, env) in &listed {
                for name in names {
                    let negation = name.starts_with("--no-") || name.starts_with("-no-");
                    assert!(
                        row.spellings.contains(&name.as_str()) || negation,
                        "--help lists {name} beside {:?}; the row misses it",
                        row.spellings
                    );
                }
                // A field's flag is always on the app's line, which beats the
                // environment (M8); every other refusal must close the side door.
                if !matches!(row.rule, Rule::Field(_) | Rule::Host) {
                    for var in env {
                        assert!(
                            row.env.contains(&var.as_str()),
                            "{var} of {names:?} is not scrubbed"
                        );
                    }
                }
            }
        }
        eprintln!("the table matches {bin}: {} options read", options.len());
    }

    /// `--help`'s options: each one's spellings (the head line's leading
    /// `-`-words) and its `(env: …)` names.
    fn help_options(help: &str) -> Vec<(Vec<String>, Vec<String>)> {
        let mut options: Vec<(Vec<String>, Vec<String>)> = Vec::new();
        for line in help.lines() {
            if line.starts_with('-') {
                let names = line
                    .split(|c: char| c.is_whitespace() || c == ',')
                    .filter(|t| !t.is_empty())
                    .take_while(|t| t.starts_with('-'))
                    .map(str::to_string)
                    .collect();
                options.push((names, Vec::new()));
            } else if let (Some(at), Some(last)) = (line.find("(env: "), options.last_mut()) {
                let var = line[at + 6..].trim_end().trim_end_matches(')');
                last.1.push(var.to_string());
            }
        }
        options
    }
}
