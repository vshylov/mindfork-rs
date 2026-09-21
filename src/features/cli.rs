//! Our own micro command-line argument parser. Replaces `clap` so that
//! **all** CLI text (help, parse errors) lives in the locale bundles (i18n,
//! axis B — docs/history/i18n-cli.md): `clap` doesn't let us localize parse-error
//! messages, and its derive attributes don't accept a locale parameter (we'd
//! have had to fall back to a global process locale — exactly what we
//! rejected `rust-i18n` for). The surface is small (a dozen subcommands, a handful
//! of options), so a custom parser follows the precedent of markdown
//! (ADR 0003), i18n, `calc`, InputBox (ADR 0001): our own solution when a
//! crate gets in the way of a requirement.
//!
//! The locale is passed **as a parameter** (no globals): [`parse`] and
//! [`render_help`] take `&Locale`, all text comes from the bundle. The
//! `Err(String)` from [`parse`] is already a print-ready message (prefix +
//! reason + a `--help` hint).
//!
//! Subcommand/flag names (`backup`, `--output`) are **not translated** — they
//! are protocol, like tool ids and the `/rag` commands (docs/history/i18n.md §2.3).

use std::path::PathBuf;

use crate::shared::i18n::Locale;

/// Default backup compression level (matches the previous `default_value_t = 9`).
pub const DEFAULT_COMPRESSION: i64 = 9;

/// A parsed CLI command. No subcommand → [`CliCommand::Run`] (launch the TUI).
#[derive(Debug, PartialEq, Eq)]
pub enum CliCommand {
    /// Launch the TUI (no arguments given).
    Run,
    /// Create a backup (`backup`).
    Backup {
        output: Option<PathBuf>,
        compression: i64,
        /// Encrypt the archive with this password (spec §12.3). `None` — fall
        /// back to the one stored in the settings, if any.
        password: Option<String>,
    },
    /// Restore from a backup (`restore <archive>`).
    Restore {
        archive: PathBuf,
        /// Password for an encrypted archive. `None` — fall back to the stored
        /// one, then to an interactive prompt.
        password: Option<String>,
    },
    /// Summarize the user data (`stats [archive]`, spec §12.4): the live data
    /// root, or — given an archive — a backup, without restoring it.
    Stats {
        archive: Option<PathBuf>,
        /// The other copy to compare with (`--compare`): a `--json` snapshot or
        /// a backup archive — which one is read from the file, not its name.
        compare: Option<PathBuf>,
        /// Password for an encrypted archive; settled like [`Self::Restore`]'s.
        /// With two archives it is tried on both.
        password: Option<String>,
        /// Print the machine-readable snapshot instead of the text.
        json: bool,
    },
    /// Import from a mindfork-import format file (`import <file>`).
    /// Format spec — docs/import-format.md.
    Import { file: PathBuf },
    /// Install the Python sandbox (`sandbox setup [--force] [--enable-python]`).
    SandboxSetup {
        force: bool,
        /// Turn `tools.python_enabled` on in `settings.json` **after** a successful
        /// provisioning (ADR 0005 §5: enabling stays a deliberate act, and never
        /// happens without the assets). Used by the Windows installer's checkbox.
        enable_python: bool,
    },
    /// List the llama.cpp backends published for this platform
    /// (`llama backends [--build <tag>]`).
    LlamaBackends { build: Option<String> },
    /// Download and unpack a llama.cpp backend (`llama setup --backend <id>`).
    /// `backend: None` — the user did not choose one, and the command prints
    /// the list instead of picking a download between 18 MB and 645 MB for them
    /// (docs/research/llama-cpp-download.md §6 F3).
    LlamaSetup {
        backend: Option<String>,
        build: Option<String>,
        /// Fetch the CUDA runtime alongside a `cuda-*` backend; `--no-cudart`
        /// turns it off for a host that already has it.
        cudart: bool,
        force: bool,
        /// Write the installed binary's path into the managed engine settings
        /// **after** a successful install. Long form only, like
        /// `--enable-python`: it writes user data.
        set_binary: bool,
    },
    /// List the llama.cpp builds already in `data/llama/` (`llama installed`).
    LlamaInstalled,
    /// Provision in one go (`setup …`): the sandbox, llama.cpp and the managed
    /// engine's settings — docs/research/cloud-provisioning.md §4.2.
    Setup(SetupArgs),
    /// Delete a downloaded build (`llama remove <id>`). `force` — delete even
    /// though a managed setting points at it.
    LlamaRemove { id: String, force: bool },
    /// Export a locale bundle (`locales export <code> --output <file>`).
    LocalesExport { code: String, output: PathBuf },
    /// Launch the interactive demo (`demo`): a throwaway data root and a
    /// scripted engine — the app without a model.
    Demo,
    /// Show help (general or for a subcommand).
    Help { topic: Option<HelpTopic> },
    /// Show the version (`-V`/`--version`).
    Version,
}

/// What `mindfork setup` was asked to do. **Every field is one optional step**,
/// and a step that was not named is not run; none at all is "nothing to do",
/// which the command answers with its help and exit code 2 (the `llama setup`
/// shape for a missing `--backend`).
///
/// Long forms only, throughout: the command writes user data, so every part of
/// it is spelled out at the call site (the `--enable-python` rule).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SetupArgs {
    /// Install the Python sandbox and switch `python_exec` on (`--sandbox`) —
    /// `sandbox setup --enable-python`.
    pub sandbox: bool,
    /// Install this llama.cpp backend, or family of one (`--llama <BACKEND>`) —
    /// `llama setup --backend`.
    pub llama: Option<String>,
    /// …from this build rather than the newest (`--llama-build <TAG>`).
    pub llama_build: Option<String>,
    /// `engine.managed.model_path`, and with it `engine.mode = managed`.
    pub model: Option<PathBuf>,
    /// `engine.managed.mmproj`.
    pub mmproj: Option<PathBuf>,
    /// `embed.managed.model_path`, and with it `embed.mode = managed`.
    pub embed_model: Option<PathBuf>,
    /// `engine.managed.context_size`.
    pub ctx: Option<u32>,
    /// `engine.managed.gpu_layers`.
    pub ngl: Option<i32>,
    /// `--set KEY=VALUE`, in the order given: any other settings field.
    pub set: Vec<(String, String)>,
    /// Start what was configured, report what it says about itself, stop it.
    pub verify: bool,
}

impl SetupArgs {
    /// No step was named.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Help topic: general (`None` for [`CliCommand::Help`]) or a specific subcommand.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum HelpTopic {
    Backup,
    Restore,
    Stats,
    Import,
    Sandbox,
    SandboxSetup,
    Llama,
    LlamaBackends,
    LlamaSetup,
    LlamaInstalled,
    LlamaRemove,
    Setup,
    Locales,
    LocalesExport,
    Demo,
}

/// Parses arguments (after the program name). `Err` — an already print-ready
/// localized message (prefix + reason + a `--help` hint).
pub fn parse(args: &[String], loc: &Locale) -> Result<CliCommand, String> {
    let toks: Vec<&str> = args.iter().map(String::as_str).collect();
    let Some((&first, rest)) = toks.split_first() else {
        return Ok(CliCommand::Run);
    };
    match first {
        "-h" | "--help" => Ok(CliCommand::Help { topic: None }),
        "-V" | "--version" => Ok(CliCommand::Version),
        "backup" => parse_backup(rest, loc),
        "restore" => parse_restore(rest, loc),
        "stats" => parse_stats(rest, loc),
        "import" => parse_import(rest, loc),
        // The command was removed (stage 1 of the "plugins" track): LameLLaMA
        // import is now done by an external converter that emits a
        // mindfork-import file. We hint at the replacement instead of a
        // generic "unknown command".
        "import-lamellama" => Err(err_line(loc, "cli.import.lamellama_removed", &[])),
        "sandbox" => parse_sandbox(rest, loc),
        "llama" => parse_llama(rest, loc),
        "setup" => parse_setup(rest, loc),
        "locales" => parse_locales(rest, loc),
        "demo" => parse_demo(rest, loc),
        other if other.starts_with('-') => Err(unknown_option(loc, other)),
        other => Err(unknown_command(loc, other)),
    }
}

fn parse_backup(toks: &[&str], loc: &Locale) -> Result<CliCommand, String> {
    let mut output = None;
    let mut compression = DEFAULT_COMPRESSION;
    let mut password = None;
    let mut i = 0;
    while i < toks.len() {
        let a = toks[i];
        if a == "-h" || a == "--help" {
            return Ok(CliCommand::Help {
                topic: Some(HelpTopic::Backup),
            });
        } else if let Some(v) = opt_value(toks, &mut i, loc, &["-o", "--output"])? {
            output = Some(PathBuf::from(v));
        } else if let Some(v) = opt_value(toks, &mut i, loc, &["-c", "--compression"])? {
            compression = parse_compression(v, loc)?;
        } else if let Some(v) = opt_value(toks, &mut i, loc, &["-p", "--password"])? {
            password = Some(v.to_string());
        } else if a.starts_with('-') {
            return Err(unknown_option(loc, a));
        } else {
            return Err(unexpected_arg(loc, a));
        }
    }
    Ok(CliCommand::Backup {
        output,
        compression,
        password,
    })
}

/// What `restore` and `stats` both take: one positional archive and its
/// password. One loop, so the two commands cannot come to spell the password
/// option differently.
struct ArchiveArgs<'a> {
    archive: Option<&'a str>,
    password: Option<String>,
    /// Which of the caller's boolean `flags` were given.
    flags: Vec<&'a str>,
    /// The value of the caller's one extra valued option, when it was given.
    extra: Option<&'a str>,
}

/// `Ok(None)` — help was asked for. `flags` — the boolean options this
/// command allows besides `-p/--password`; `valued` — the aliases of its one
/// extra option that takes a value (empty — it has none).
fn archive_args<'a>(
    toks: &[&'a str],
    loc: &Locale,
    flags: &[&str],
    valued: &[&str],
) -> Result<Option<ArchiveArgs<'a>>, String> {
    let mut args = ArchiveArgs {
        archive: None,
        password: None,
        flags: Vec::new(),
        extra: None,
    };
    let mut i = 0;
    while i < toks.len() {
        let a = toks[i];
        if a == "-h" || a == "--help" {
            return Ok(None);
        } else if let Some(v) = opt_value(toks, &mut i, loc, &["-p", "--password"])? {
            args.password = Some(v.to_string());
        } else if let Some(v) = opt_value(toks, &mut i, loc, valued)? {
            args.extra = Some(v);
        } else if flags.contains(&a) {
            args.flags.push(a);
            i += 1;
        } else if a.starts_with('-') {
            return Err(unknown_option(loc, a));
        } else if args.archive.is_none() {
            args.archive = Some(a);
            i += 1;
        } else {
            return Err(unexpected_arg(loc, a));
        }
    }
    Ok(Some(args))
}

fn parse_restore(toks: &[&str], loc: &Locale) -> Result<CliCommand, String> {
    let Some(args) = archive_args(toks, loc, &[], &[])? else {
        return Ok(CliCommand::Help {
            topic: Some(HelpTopic::Restore),
        });
    };
    Ok(CliCommand::Restore {
        archive: PathBuf::from(args.archive.ok_or_else(|| missing_arg(loc, "<archive>"))?),
        password: args.password,
    })
}

fn parse_stats(toks: &[&str], loc: &Locale) -> Result<CliCommand, String> {
    const JSON: &str = "--json";
    let Some(args) = archive_args(toks, loc, &[JSON], &["--compare"])? else {
        return Ok(CliCommand::Help {
            topic: Some(HelpTopic::Stats),
        });
    };
    // A password with nothing to open is a mistyped command, not a harmless
    // extra: the likeliest cause is a forgotten archive path, and summarizing
    // the live data instead would answer a question nobody asked.
    // (`--compare` may name an archive too, and then the password is for it.)
    if args.password.is_some() && args.archive.is_none() && args.extra.is_none() {
        return Err(err_line(
            loc,
            "cli.parse.stats_password_without_archive",
            &[],
        ));
    }
    Ok(CliCommand::Stats {
        archive: args.archive.map(PathBuf::from),
        compare: args.extra.map(PathBuf::from),
        password: args.password,
        json: args.flags.contains(&JSON),
    })
}

fn parse_import(toks: &[&str], loc: &Locale) -> Result<CliCommand, String> {
    match single_positional(toks, loc, HelpTopic::Import, "<file>")? {
        Positional::Help => Ok(CliCommand::Help {
            topic: Some(HelpTopic::Import),
        }),
        Positional::Value(file) => Ok(CliCommand::Import {
            file: PathBuf::from(file),
        }),
    }
}

fn parse_sandbox(toks: &[&str], loc: &Locale) -> Result<CliCommand, String> {
    let Some((&sub, rest)) = toks.split_first() else {
        return Err(missing_subcommand(loc, "sandbox"));
    };
    match sub {
        "-h" | "--help" => Ok(CliCommand::Help {
            topic: Some(HelpTopic::Sandbox),
        }),
        "setup" => parse_sandbox_setup(rest, loc),
        other if other.starts_with('-') => Err(unknown_option(loc, other)),
        other => Err(unknown_subcommand(loc, other, "sandbox")),
    }
}

fn parse_sandbox_setup(toks: &[&str], loc: &Locale) -> Result<CliCommand, String> {
    let mut force = false;
    let mut enable_python = false;
    for &a in toks {
        match a {
            "-h" | "--help" => {
                return Ok(CliCommand::Help {
                    topic: Some(HelpTopic::SandboxSetup),
                });
            }
            "-f" | "--force" => force = true,
            // Long form only, deliberately: it changes a security-relevant setting,
            // so it should be spelled out at the call site.
            "--enable-python" => enable_python = true,
            _ if a.starts_with('-') => return Err(unknown_option(loc, a)),
            _ => return Err(unexpected_arg(loc, a)),
        }
    }
    Ok(CliCommand::SandboxSetup {
        force,
        enable_python,
    })
}

fn parse_llama(toks: &[&str], loc: &Locale) -> Result<CliCommand, String> {
    let Some((&sub, rest)) = toks.split_first() else {
        return Err(missing_subcommand(loc, "llama"));
    };
    match sub {
        "-h" | "--help" => Ok(CliCommand::Help {
            topic: Some(HelpTopic::Llama),
        }),
        "backends" => parse_llama_backends(rest, loc),
        "setup" => parse_llama_setup(rest, loc),
        "installed" => parse_llama_installed(rest, loc),
        "remove" => parse_llama_remove(rest, loc),
        other if other.starts_with('-') => Err(unknown_option(loc, other)),
        other => Err(unknown_subcommand(loc, other, "llama")),
    }
}

fn parse_llama_backends(toks: &[&str], loc: &Locale) -> Result<CliCommand, String> {
    let mut build = None;
    let mut i = 0;
    while i < toks.len() {
        let a = toks[i];
        if a == "-h" || a == "--help" {
            return Ok(CliCommand::Help {
                topic: Some(HelpTopic::LlamaBackends),
            });
        } else if let Some(v) = opt_value(toks, &mut i, loc, &["--build"])? {
            build = Some(v.to_string());
        } else if a.starts_with('-') {
            return Err(unknown_option(loc, a));
        } else {
            return Err(unexpected_arg(loc, a));
        }
    }
    Ok(CliCommand::LlamaBackends { build })
}

fn parse_llama_setup(toks: &[&str], loc: &Locale) -> Result<CliCommand, String> {
    let mut backend = None;
    let mut build = None;
    let mut cudart = true;
    let mut force = false;
    let mut set_binary = false;
    let mut i = 0;
    while i < toks.len() {
        let a = toks[i];
        if a == "-h" || a == "--help" {
            return Ok(CliCommand::Help {
                topic: Some(HelpTopic::LlamaSetup),
            });
        } else if let Some(v) = opt_value(toks, &mut i, loc, &["-b", "--backend"])? {
            backend = Some(v.to_string());
        } else if let Some(v) = opt_value(toks, &mut i, loc, &["--build"])? {
            build = Some(v.to_string());
        } else if a == "-f" || a == "--force" {
            force = true;
            i += 1;
        } else if a == "--no-cudart" {
            cudart = false;
            i += 1;
        } else if a == "--set-binary" {
            // Long form only, deliberately: like `--enable-python`, it writes
            // to `settings.json`, so it should be spelled out at the call site.
            set_binary = true;
            i += 1;
        } else if a.starts_with('-') {
            return Err(unknown_option(loc, a));
        } else {
            return Err(unexpected_arg(loc, a));
        }
    }
    Ok(CliCommand::LlamaSetup {
        backend,
        build,
        cudart,
        force,
        set_binary,
    })
}

fn parse_llama_installed(toks: &[&str], loc: &Locale) -> Result<CliCommand, String> {
    match toks.first() {
        None => Ok(CliCommand::LlamaInstalled),
        Some(&("-h" | "--help")) => Ok(CliCommand::Help {
            topic: Some(HelpTopic::LlamaInstalled),
        }),
        Some(&a) if a.starts_with('-') => Err(unknown_option(loc, a)),
        Some(&a) => Err(unexpected_arg(loc, a)),
    }
}

fn parse_llama_remove(toks: &[&str], loc: &Locale) -> Result<CliCommand, String> {
    let mut id: Option<String> = None;
    let mut force = false;
    for &a in toks {
        match a {
            "-h" | "--help" => {
                return Ok(CliCommand::Help {
                    topic: Some(HelpTopic::LlamaRemove),
                });
            }
            "-f" | "--force" => force = true,
            _ if a.starts_with('-') => return Err(unknown_option(loc, a)),
            _ if id.is_none() => id = Some(a.to_string()),
            _ => return Err(unexpected_arg(loc, a)),
        }
    }
    // The build to delete is never defaulted: this frees up to a gigabyte and
    // there is no undo.
    let id = id.ok_or_else(|| missing_arg(loc, "<id>"))?;
    Ok(CliCommand::LlamaRemove { id, force })
}

fn parse_setup(toks: &[&str], loc: &Locale) -> Result<CliCommand, String> {
    let mut args = SetupArgs::default();
    let mut i = 0;
    while i < toks.len() {
        let a = toks[i];
        match a {
            "-h" | "--help" => {
                return Ok(CliCommand::Help {
                    topic: Some(HelpTopic::Setup),
                });
            }
            "--sandbox" => {
                args.sandbox = true;
                i += 1;
            }
            "--verify" => {
                args.verify = true;
                i += 1;
            }
            _ => match setup_value(toks, &mut i, loc)? {
                Some((opt, v)) => args.assign(opt, v, loc)?,
                None if a.starts_with('-') => return Err(unknown_option(loc, a)),
                None => return Err(unexpected_arg(loc, a)),
            },
        }
    }
    // A build with no backend to take it from is a mistyped command, not a
    // harmless extra: the likeliest cause is a forgotten `--llama`.
    if args.llama_build.is_some() && args.llama.is_none() {
        return Err(missing_opt(loc, "--llama"));
    }
    Ok(CliCommand::Setup(args))
}

/// The options of `setup` that take a value.
const SETUP_VALUED: &[&str] = &[
    "--llama",
    "--llama-build",
    "--model",
    "--mmproj",
    "--embed-model",
    "--ctx",
    "--ngl",
    "--set",
];

/// The valued option at `toks[*i]` and its value, when it is one of `setup`'s.
fn setup_value<'a>(
    toks: &[&'a str],
    i: &mut usize,
    loc: &Locale,
) -> Result<Option<(&'static str, &'a str)>, String> {
    for &opt in SETUP_VALUED {
        if let Some(v) = opt_value(toks, i, loc, &[opt])? {
            return Ok(Some((opt, v)));
        }
    }
    Ok(None)
}

impl SetupArgs {
    /// Stores the value of one of [`SETUP_VALUED`].
    fn assign(&mut self, opt: &str, v: &str, loc: &Locale) -> Result<(), String> {
        match opt {
            "--llama" => self.llama = Some(v.to_string()),
            "--llama-build" => self.llama_build = Some(v.to_string()),
            "--model" => self.model = Some(PathBuf::from(v)),
            "--mmproj" => self.mmproj = Some(PathBuf::from(v)),
            "--embed-model" => self.embed_model = Some(PathBuf::from(v)),
            "--ctx" => self.ctx = Some(parse_number(v, opt, loc)?),
            "--ngl" => self.ngl = Some(parse_number(v, opt, loc)?),
            "--set" => self.set.push(parse_assignment(v, loc)?),
            other => unreachable!("not one of SETUP_VALUED: {other}"),
        }
        Ok(())
    }
}

/// A numeric option's value, or a refusal naming the option and what was typed.
fn parse_number<T: std::str::FromStr>(v: &str, opt: &str, loc: &Locale) -> Result<T, String> {
    v.trim()
        .parse()
        .map_err(|_| err_line(loc, "cli.parse.bad_number", &[("opt", opt), ("value", v)]))
}

/// `KEY=VALUE`, split at the **first** `=` — the value may carry more of them
/// (a URL with a query, a JSON literal).
fn parse_assignment(v: &str, loc: &Locale) -> Result<(String, String), String> {
    match v.split_once('=') {
        Some((key, value)) if !key.trim().is_empty() => {
            Ok((key.trim().to_string(), value.to_string()))
        }
        _ => Err(err_line(loc, "cli.parse.bad_set", &[("value", v)])),
    }
}

fn parse_demo(toks: &[&str], loc: &Locale) -> Result<CliCommand, String> {
    // `demo` takes no arguments; the first token decides everything.
    match toks.first() {
        None => Ok(CliCommand::Demo),
        Some(&("-h" | "--help")) => Ok(CliCommand::Help {
            topic: Some(HelpTopic::Demo),
        }),
        Some(&a) if a.starts_with('-') => Err(unknown_option(loc, a)),
        Some(&a) => Err(unexpected_arg(loc, a)),
    }
}

fn parse_locales(toks: &[&str], loc: &Locale) -> Result<CliCommand, String> {
    let Some((&sub, rest)) = toks.split_first() else {
        return Err(missing_subcommand(loc, "locales"));
    };
    match sub {
        "-h" | "--help" => Ok(CliCommand::Help {
            topic: Some(HelpTopic::Locales),
        }),
        "export" => parse_locales_export(rest, loc),
        other if other.starts_with('-') => Err(unknown_option(loc, other)),
        other => Err(unknown_subcommand(loc, other, "locales")),
    }
}

fn parse_locales_export(toks: &[&str], loc: &Locale) -> Result<CliCommand, String> {
    let mut code: Option<String> = None;
    let mut output: Option<PathBuf> = None;
    let mut i = 0;
    while i < toks.len() {
        let a = toks[i];
        if a == "-h" || a == "--help" {
            return Ok(CliCommand::Help {
                topic: Some(HelpTopic::LocalesExport),
            });
        } else if let Some(v) = opt_value(toks, &mut i, loc, &["-o", "--output"])? {
            output = Some(PathBuf::from(v));
        } else if a.starts_with('-') {
            return Err(unknown_option(loc, a));
        } else if code.is_none() {
            code = Some(a.to_string());
            i += 1;
        } else {
            return Err(unexpected_arg(loc, a));
        }
    }
    let code = code.ok_or_else(|| missing_arg(loc, "<code>"))?;
    let output = output.ok_or_else(|| missing_opt(loc, "--output"))?;
    Ok(CliCommand::LocalesExport { code, output })
}

// -------- Parsing a single required positional argument (restore/import) --------

enum Positional<'a> {
    Help,
    Value(&'a str),
}

fn single_positional<'a>(
    toks: &[&'a str],
    loc: &Locale,
    _topic: HelpTopic,
    arg_name: &str,
) -> Result<Positional<'a>, String> {
    let mut value: Option<&'a str> = None;
    for &a in toks {
        if a == "-h" || a == "--help" {
            return Ok(Positional::Help);
        } else if a.starts_with('-') {
            return Err(unknown_option(loc, a));
        } else if value.is_none() {
            value = Some(a);
        } else {
            return Err(unexpected_arg(loc, a));
        }
    }
    match value {
        Some(v) => Ok(Positional::Value(v)),
        None => Err(missing_arg(loc, arg_name)),
    }
}

// -------- An option with a value: `--opt value` and `--opt=value` (both forms + the short one) --------

/// If `toks[*i]` matches one of `aliases`, extracts the value (the next
/// token for the `--opt value` form, or the part after `=` for
/// `--opt=value`), advances `*i`, and returns `Ok(Some(value))`. If the token
/// matched none of the aliases — `Ok(None)` (the index doesn't move). It
/// matched but there's no value — `Err`.
fn opt_value<'a>(
    toks: &[&'a str],
    i: &mut usize,
    loc: &Locale,
    aliases: &[&str],
) -> Result<Option<&'a str>, String> {
    let a = toks[*i];
    // The `--opt value` form: an exact name match, the value is the next token.
    if aliases.contains(&a) {
        let display = aliases.last().copied().unwrap_or(a);
        let v = *toks
            .get(*i + 1)
            .ok_or_else(|| missing_value(loc, display))?;
        *i += 2;
        return Ok(Some(v));
    }
    // The `--opt=value` / `-o=value` form.
    for al in aliases {
        if let Some(v) = a.strip_prefix(al).and_then(|r| r.strip_prefix('=')) {
            *i += 1;
            return Ok(Some(v));
        }
    }
    Ok(None)
}

fn parse_compression(v: &str, loc: &Locale) -> Result<i64, String> {
    match v.parse::<i64>() {
        Ok(n) if (0..=9).contains(&n) => Ok(n),
        _ => Err(err_line(loc, "cli.parse.bad_compression", &[("value", v)])),
    }
}

// -------- Error-message constructors (print-ready text) --------

/// The full error line: `{prefix}: {reason}\n\n{hint about --help}`.
fn err_line(loc: &Locale, key: &str, args: &[(&str, &str)]) -> String {
    format!(
        "{}: {}\n\n{}",
        loc.t("cli.err.prefix"),
        loc.tf(key, args),
        loc.t("cli.help.try_help")
    )
}

fn unknown_command(loc: &Locale, cmd: &str) -> String {
    err_line(loc, "cli.parse.unknown_command", &[("cmd", cmd)])
}

fn unknown_subcommand(loc: &Locale, sub: &str, cmd: &str) -> String {
    err_line(
        loc,
        "cli.parse.unknown_subcommand",
        &[("sub", sub), ("cmd", cmd)],
    )
}

fn missing_subcommand(loc: &Locale, cmd: &str) -> String {
    err_line(loc, "cli.parse.missing_subcommand", &[("cmd", cmd)])
}

fn unknown_option(loc: &Locale, opt: &str) -> String {
    err_line(loc, "cli.parse.unknown_option", &[("opt", opt)])
}

fn unexpected_arg(loc: &Locale, arg: &str) -> String {
    err_line(loc, "cli.parse.unexpected_arg", &[("arg", arg)])
}

fn missing_value(loc: &Locale, opt: &str) -> String {
    err_line(loc, "cli.parse.missing_value", &[("opt", opt)])
}

fn missing_arg(loc: &Locale, arg: &str) -> String {
    err_line(loc, "cli.parse.missing_arg", &[("arg", arg)])
}

fn missing_opt(loc: &Locale, opt: &str) -> String {
    err_line(loc, "cli.parse.missing_opt", &[("opt", opt)])
}

// -------- Help --------

/// Help text: general (`topic == None`) or for a subcommand. All labels come
/// from the bundle; command/flag names and argument placeholders
/// (`<archive>`) are protocol and aren't translated.
pub fn render_help(topic: Option<HelpTopic>, loc: &Locale) -> String {
    let usage = loc.t("cli.help.usage");
    let commands = loc.t("cli.help.commands");
    let options = loc.t("cli.help.options");
    let arguments = loc.t("cli.help.arguments");
    match topic {
        None => format!(
            "{about}\n\n{usage} mindfork [COMMAND]\n\n{commands}\n\
             {dm:<20}{cd}\n{su:<20}{csu}\n{b:<20}{cb}\n{r:<20}{cr}\n{st:<20}{cst}\n{im:<20}{ci}\n{sb:<20}{cs}\n{ll:<20}{cll}\n{lc:<20}{cl}\n\n\
             {options}\n  -h, --help     {oh}\n  -V, --version  {ov}",
            about = loc.t("cli.help.about"),
            dm = "  demo",
            cd = loc.t("cli.help.cmd.demo"),
            su = "  setup",
            csu = loc.t("cli.help.cmd.setup"),
            b = "  backup",
            cb = loc.t("cli.help.cmd.backup"),
            r = "  restore",
            cr = loc.t("cli.help.cmd.restore"),
            st = "  stats",
            cst = loc.t("cli.help.cmd.stats"),
            im = "  import",
            ci = loc.t("cli.help.cmd.import"),
            sb = "  sandbox",
            cs = loc.t("cli.help.cmd.sandbox"),
            ll = "  llama",
            cll = loc.t("cli.help.cmd.llama"),
            lc = "  locales",
            cl = loc.t("cli.help.cmd.locales"),
            oh = loc.t("cli.help.opt.help"),
            ov = loc.t("cli.help.opt.version"),
        ),
        Some(HelpTopic::Backup) => format!(
            "{d}\n\n{usage} mindfork backup [OPTIONS]\n\n{options}\n\
             {o:<30}{co}\n{c:<30}{cc}\n{p:<30}{cp}\n{h:<30}{ch}",
            d = loc.t("cli.help.cmd.backup"),
            o = "  -o, --output <FILE>",
            co = loc.t("cli.help.opt.backup.output"),
            c = "  -c, --compression <0-9>",
            cc = loc.t("cli.help.opt.backup.compression"),
            p = "  -p, --password <PASSWORD>",
            cp = loc.t("cli.help.opt.backup.password"),
            h = "  -h, --help",
            ch = loc.t("cli.help.opt.help"),
        ),
        Some(HelpTopic::Restore) => format!(
            "{d}\n\n{usage} mindfork restore <ARCHIVE> [OPTIONS]\n\n{arguments}\n\
             {a:<30}{ca}\n\n{options}\n{p:<30}{cp}\n{h:<30}{ch}",
            d = loc.t("cli.help.cmd.restore"),
            a = "  <ARCHIVE>",
            ca = loc.t("cli.help.arg.restore.archive"),
            p = "  -p, --password <PASSWORD>",
            cp = loc.t("cli.help.opt.restore.password"),
            h = "  -h, --help",
            ch = loc.t("cli.help.opt.help"),
        ),
        Some(HelpTopic::Stats) => format!(
            "{d}\n\n{usage} mindfork stats [ARCHIVE] [OPTIONS]\n\n{arguments}\n\
             {a:<30}{ca}\n\n{options}\n{c:<30}{cc}\n{p:<30}{cp}\n{j:<30}{cj}\n{h:<30}{ch}\n\n{n}",
            d = loc.t("cli.help.cmd.stats"),
            a = "  [ARCHIVE]",
            ca = loc.t("cli.help.arg.stats.archive"),
            c = "  --compare <OTHER>",
            cc = loc.t("cli.help.opt.stats.compare"),
            p = "  -p, --password <PASSWORD>",
            cp = loc.t("cli.help.opt.restore.password"),
            j = "  --json",
            cj = loc.t("cli.help.opt.stats.json"),
            h = "  -h, --help",
            ch = loc.t("cli.help.opt.help"),
            n = loc.t("cli.help.stats.note"),
        ),
        Some(HelpTopic::Import) => format!(
            "{d}\n\n{usage} mindfork import <FILE>\n\n{arguments}\n\
             {a:<12}{ca}",
            d = loc.t("cli.help.cmd.import"),
            a = "  <FILE>",
            ca = loc.t("cli.help.arg.import.file"),
        ),
        Some(HelpTopic::Sandbox) => format!(
            "{d}\n\n{usage} mindfork sandbox <COMMAND>\n\n{commands}\n{s:<12}{cs}",
            d = loc.t("cli.help.cmd.sandbox"),
            s = "  setup",
            cs = loc.t("cli.help.cmd.sandbox.setup"),
        ),
        Some(HelpTopic::SandboxSetup) => format!(
            "{d}\n\n{usage} mindfork sandbox setup [OPTIONS]\n\n{options}\n\
             {f:<20}{cf}\n{e:<20}{ce}\n{h:<20}{ch}",
            d = loc.t("cli.help.cmd.sandbox.setup"),
            f = "  -f, --force",
            cf = loc.t("cli.help.opt.sandbox.force"),
            e = "  --enable-python",
            ce = loc.t("cli.help.opt.sandbox.enable_python"),
            h = "  -h, --help",
            ch = loc.t("cli.help.opt.help"),
        ),
        Some(HelpTopic::Llama) => format!(
            "{d}\n\n{usage} mindfork llama <COMMAND>\n\n{commands}\n\
             {b:<14}{cb}\n{s:<14}{cs}\n{i:<14}{ci}\n{r:<14}{cr}",
            d = loc.t("cli.help.cmd.llama"),
            b = "  backends",
            cb = loc.t("cli.help.cmd.llama.backends"),
            s = "  setup",
            cs = loc.t("cli.help.cmd.llama.setup"),
            i = "  installed",
            ci = loc.t("cli.help.cmd.llama.installed"),
            r = "  remove",
            cr = loc.t("cli.help.cmd.llama.remove"),
        ),
        Some(HelpTopic::LlamaBackends) => format!(
            "{d}\n\n{usage} mindfork llama backends [OPTIONS]\n\n{options}\n\
             {b:<24}{cb}\n{h:<24}{ch}",
            d = loc.t("cli.help.cmd.llama.backends"),
            b = "  --build <TAG>",
            cb = loc.t("cli.help.opt.llama.build"),
            h = "  -h, --help",
            ch = loc.t("cli.help.opt.help"),
        ),
        Some(HelpTopic::LlamaSetup) => format!(
            "{d}\n\n{usage} mindfork llama setup --backend <ID> [OPTIONS]\n\n{options}\n\
             {b:<24}{cb}\n{bu:<24}{cbu}\n{sb:<24}{csb}\n{n:<24}{cn}\n{f:<24}{cf}\n{h:<24}{ch}",
            d = loc.t("cli.help.cmd.llama.setup"),
            b = "  -b, --backend <ID>",
            cb = loc.t("cli.help.opt.llama.backend"),
            bu = "  --build <TAG>",
            cbu = loc.t("cli.help.opt.llama.build"),
            sb = "  --set-binary",
            csb = loc.t("cli.help.opt.llama.set_binary"),
            n = "  --no-cudart",
            cn = loc.t("cli.help.opt.llama.no_cudart"),
            f = "  -f, --force",
            cf = loc.t("cli.help.opt.llama.force"),
            h = "  -h, --help",
            ch = loc.t("cli.help.opt.help"),
        ),
        Some(HelpTopic::LlamaInstalled) => format!(
            "{d}\n\n{usage} mindfork llama installed",
            d = loc.t("cli.help.cmd.llama.installed"),
        ),
        Some(HelpTopic::LlamaRemove) => format!(
            "{d}\n\n{usage} mindfork llama remove <ID> [OPTIONS]\n\n\
             {arguments}\n{a:<20}{ca}\n\n{options}\n{f:<20}{cf}\n{h:<20}{ch}",
            d = loc.t("cli.help.cmd.llama.remove"),
            a = "  <ID>",
            ca = loc.t("cli.help.arg.llama.id"),
            f = "  -f, --force",
            cf = loc.t("cli.help.opt.llama.remove_force"),
            h = "  -h, --help",
            ch = loc.t("cli.help.opt.help"),
        ),
        Some(HelpTopic::Setup) => format!(
            "{d}\n\n{usage} mindfork setup [OPTIONS]\n\n{options}\n\
             {sb:<26}{csb}\n{ll:<26}{cll}\n{lb:<26}{clb}\n{m:<26}{cm}\n{mp:<26}{cmp}\n\
             {em:<26}{cem}\n{cx:<26}{ccx}\n{ng:<26}{cng}\n{st:<26}{cst}\n{vf:<26}{cvf}\n\
             {h:<26}{ch}\n\n{n}",
            d = loc.t("cli.help.cmd.setup"),
            sb = "  --sandbox",
            csb = loc.t("cli.help.opt.setup.sandbox"),
            ll = "  --llama <BACKEND>",
            cll = loc.t("cli.help.opt.setup.llama"),
            lb = "  --llama-build <TAG>",
            clb = loc.t("cli.help.opt.llama.build"),
            m = "  --model <GGUF>",
            cm = loc.t("cli.help.opt.setup.model"),
            mp = "  --mmproj <GGUF>",
            cmp = loc.t("cli.help.opt.setup.mmproj"),
            em = "  --embed-model <GGUF>",
            cem = loc.t("cli.help.opt.setup.embed_model"),
            cx = "  --ctx <N>",
            ccx = loc.t("cli.help.opt.setup.ctx"),
            ng = "  --ngl <N>",
            cng = loc.t("cli.help.opt.setup.ngl"),
            st = "  --set <KEY>=<VALUE>",
            cst = loc.t("cli.help.opt.setup.set"),
            vf = "  --verify",
            cvf = loc.t("cli.help.opt.setup.verify"),
            h = "  -h, --help",
            ch = loc.t("cli.help.opt.help"),
            n = loc.t("cli.help.setup.note"),
        ),
        Some(HelpTopic::Demo) => format!(
            "{d}\n\n{usage} mindfork demo\n\n{n}",
            d = loc.t("cli.help.cmd.demo"),
            n = loc.t("cli.help.demo.note"),
        ),
        Some(HelpTopic::Locales) => format!(
            "{d}\n\n{usage} mindfork locales <COMMAND>\n\n{commands}\n{e:<12}{ce}",
            d = loc.t("cli.help.cmd.locales"),
            e = "  export",
            ce = loc.t("cli.help.cmd.locales.export"),
        ),
        Some(HelpTopic::LocalesExport) => format!(
            "{d}\n\n{usage} mindfork locales export <CODE> --output <FILE>\n\n\
             {arguments}\n{a:<24}{ca}\n\n{options}\n{o:<24}{co}\n{h:<24}{ch}",
            d = loc.t("cli.help.cmd.locales.export"),
            a = "  <CODE>",
            ca = loc.t("cli.help.arg.locales.code"),
            o = "  -o, --output <FILE>",
            co = loc.t("cli.help.opt.locales.output"),
            h = "  -h, --help",
            ch = loc.t("cli.help.opt.help"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::i18n::{Lang, locale};

    fn p(args: &[&str]) -> Result<CliCommand, String> {
        let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        parse(&owned, locale(Lang::Ru))
    }

    #[test]
    fn no_args_is_run() {
        assert_eq!(p(&[]).unwrap(), CliCommand::Run);
    }

    /// `demo` takes no arguments: bare → the command, `--help` → its topic,
    /// anything else → a localized refusal, not a silent ignore.
    #[test]
    fn demo_parses_bare_and_help_only() {
        assert_eq!(p(&["demo"]).unwrap(), CliCommand::Demo);
        assert_eq!(
            p(&["demo", "--help"]).unwrap(),
            CliCommand::Help {
                topic: Some(HelpTopic::Demo)
            }
        );
        assert!(p(&["demo", "--force"]).is_err(), "unknown option refused");
        assert!(p(&["demo", "extra"]).is_err(), "stray argument refused");
    }

    /// The demo appears in the general help and has a topic page.
    #[test]
    fn demo_help_is_rendered() {
        let loc = locale(Lang::En);
        let general = render_help(None, loc);
        assert!(general.contains("demo"), "listed in the command table");
        let topic = render_help(Some(HelpTopic::Demo), loc);
        assert!(topic.contains("mindfork demo"));
        assert!(
            topic.contains("temporary folder"),
            "the note must say where the data lives"
        );
    }

    #[test]
    fn global_help_and_version() {
        assert_eq!(p(&["--help"]).unwrap(), CliCommand::Help { topic: None });
        assert_eq!(p(&["-h"]).unwrap(), CliCommand::Help { topic: None });
        assert_eq!(p(&["--version"]).unwrap(), CliCommand::Version);
        assert_eq!(p(&["-V"]).unwrap(), CliCommand::Version);
    }

    #[test]
    fn backup_defaults_and_options() {
        assert_eq!(
            p(&["backup"]).unwrap(),
            CliCommand::Backup {
                output: None,
                compression: DEFAULT_COMPRESSION,
                password: None,
            }
        );
        // Both option forms: `--opt value` and `--opt=value`, plus the short `-o`.
        assert_eq!(
            p(&["backup", "-o", "a.zip", "-c", "3"]).unwrap(),
            CliCommand::Backup {
                output: Some(PathBuf::from("a.zip")),
                compression: 3,
                password: None,
            }
        );
        assert_eq!(
            p(&["backup", "--output=b.zip", "--compression=0"]).unwrap(),
            CliCommand::Backup {
                output: Some(PathBuf::from("b.zip")),
                compression: 0,
                password: None,
            }
        );
    }

    #[test]
    fn backup_and_restore_take_a_password() {
        // Both spellings, and on both commands — `restore` had to grow its own
        // option parsing (it used to reject anything starting with `-`).
        assert_eq!(
            p(&["backup", "-p", "s3cret"]).unwrap(),
            CliCommand::Backup {
                output: None,
                compression: DEFAULT_COMPRESSION,
                password: Some("s3cret".into()),
            }
        );
        assert_eq!(
            p(&["restore", "a.zip", "--password=s3cret"]).unwrap(),
            CliCommand::Restore {
                archive: PathBuf::from("a.zip"),
                password: Some("s3cret".into()),
            }
        );
        // The option may precede the positional argument.
        assert_eq!(
            p(&["restore", "--password", "s3cret", "a.zip"]).unwrap(),
            CliCommand::Restore {
                archive: PathBuf::from("a.zip"),
                password: Some("s3cret".into()),
            }
        );
        assert!(p(&["restore", "-p"]).is_err()); // option with no value
    }

    #[test]
    fn backup_bad_compression() {
        assert!(p(&["backup", "-c", "10"]).is_err());
        assert!(p(&["backup", "-c", "x"]).is_err());
    }

    #[test]
    fn restore_requires_archive() {
        assert_eq!(
            p(&["restore", "a.zip"]).unwrap(),
            CliCommand::Restore {
                archive: PathBuf::from("a.zip"),
                password: None,
            }
        );
        assert!(p(&["restore"]).is_err()); // no required argument
        assert!(p(&["restore", "a.zip", "b.zip"]).is_err()); // extra argument
    }

    /// `stats` alone reads the live data; an archive, its password (both
    /// spellings, either order) and `--json` are all optional.
    #[test]
    fn stats_takes_an_optional_archive_a_password_and_json() {
        let stats = |archive: Option<&str>, password: Option<&str>, json| CliCommand::Stats {
            archive: archive.map(PathBuf::from),
            compare: None,
            password: password.map(str::to_string),
            json,
        };
        assert_eq!(p(&["stats"]).unwrap(), stats(None, None, false));
        assert_eq!(p(&["stats", "--json"]).unwrap(), stats(None, None, true));
        assert_eq!(
            p(&["stats", "a.zip"]).unwrap(),
            stats(Some("a.zip"), None, false)
        );
        assert_eq!(
            p(&["stats", "--json", "-p", "s3cret", "a.zip"]).unwrap(),
            stats(Some("a.zip"), Some("s3cret"), true)
        );
        assert_eq!(
            p(&["stats", "a.zip", "--password=s3cret"]).unwrap(),
            stats(Some("a.zip"), Some("s3cret"), false)
        );
        assert_eq!(
            p(&["stats", "a.zip", "--help"]).unwrap(),
            CliCommand::Help {
                topic: Some(HelpTopic::Stats)
            }
        );
    }

    /// `--compare` takes the other copy — both spellings, anywhere on the
    /// line — and composes with the archive, the password and `--json`.
    #[test]
    fn stats_compare_names_the_other_copy() {
        let compare =
            |archive: Option<&str>, other: &str, password: Option<&str>, json| CliCommand::Stats {
                archive: archive.map(PathBuf::from),
                compare: Some(PathBuf::from(other)),
                password: password.map(str::to_string),
                json,
            };
        assert_eq!(
            p(&["stats", "--compare", "b.json"]).unwrap(),
            compare(None, "b.json", None, false)
        );
        assert_eq!(
            p(&["stats", "--compare=b.zip", "a.zip", "--json"]).unwrap(),
            compare(Some("a.zip"), "b.zip", None, true)
        );
        // A password with only the other copy to open is for that copy.
        assert_eq!(
            p(&["stats", "-p", "s3cret", "--compare", "b.zip"]).unwrap(),
            compare(None, "b.zip", Some("s3cret"), false)
        );
        assert!(
            p(&["stats", "--compare"]).is_err(),
            "the option needs a value"
        );
        // The option is `stats`'s own: `restore` still refuses it.
        assert!(p(&["restore", "a.zip", "--compare", "b.zip"]).is_err());
    }

    #[test]
    fn stats_refuses_what_it_cannot_mean() {
        assert!(p(&["stats", "a.zip", "b.zip"]).is_err(), "one archive");
        assert!(
            p(&["stats", "-p"]).is_err(),
            "a password option with no value"
        );
        // A password and no archive: refused, and the refusal says why — in
        // both locales, since it is the one error this command owns.
        for lang in [Lang::En, Lang::Ru] {
            let owned = ["stats".to_string(), "-p".to_string(), "x".to_string()];
            let err = parse(&owned, locale(lang)).unwrap_err();
            assert!(
                err.contains(locale(lang).t("cli.parse.stats_password_without_archive")),
                "{lang:?}: {err}"
            );
        }
        // `--json` belongs to `stats` alone: `restore` still refuses it.
        assert!(p(&["restore", "a.zip", "--json"]).is_err());
    }

    /// The topic says the one thing a user deciding whether to run it on a
    /// live, precious data root needs to hear: it only reads.
    #[test]
    fn stats_help_is_listed_and_says_it_only_reads() {
        let loc = locale(Lang::En);
        assert!(render_help(None, loc).contains("stats  "));
        let topic = render_help(Some(HelpTopic::Stats), loc);
        assert!(
            topic.contains("mindfork stats [ARCHIVE] [OPTIONS]"),
            "{topic}"
        );
        assert!(topic.contains("--password <PASSWORD>  "), "{topic}");
        assert!(topic.contains("--json"), "{topic}");
        assert!(topic.contains("--compare <OTHER>  "), "{topic}");
        assert!(topic.contains("only reads"), "{topic}");
    }

    #[test]
    fn import_requires_file() {
        assert_eq!(
            p(&["import", "f.json"]).unwrap(),
            CliCommand::Import {
                file: PathBuf::from("f.json")
            }
        );
        assert!(p(&["import"]).is_err());
    }

    #[test]
    fn import_lamellama_hints_replacement() {
        // The removed command hints at the external converter + `import`,
        // rather than a generic "unknown command".
        let err = p(&["import-lamellama", "d"]).unwrap_err();
        assert!(err.contains("import"), "{err}");
        assert!(err.contains("mindfork-import"), "{err}");
    }

    #[test]
    fn sandbox_setup_force() {
        assert_eq!(
            p(&["sandbox", "setup"]).unwrap(),
            CliCommand::SandboxSetup {
                force: false,
                enable_python: false,
            }
        );
        assert_eq!(
            p(&["sandbox", "setup", "--force"]).unwrap(),
            CliCommand::SandboxSetup {
                force: true,
                enable_python: false,
            }
        );
        assert!(p(&["sandbox"]).is_err()); // no subcommand
        assert!(p(&["sandbox", "teardown"]).is_err()); // unknown subcommand
    }

    #[test]
    fn sandbox_setup_enable_python() {
        // The flag the Windows installer's checkbox passes. Off unless asked for —
        // the default must never enable the tool (ADR 0005 §5).
        assert_eq!(
            p(&["sandbox", "setup", "--enable-python"]).unwrap(),
            CliCommand::SandboxSetup {
                force: false,
                enable_python: true,
            }
        );
        assert_eq!(
            p(&["sandbox", "setup", "--force", "--enable-python"]).unwrap(),
            CliCommand::SandboxSetup {
                force: true,
                enable_python: true,
            }
        );
        // Long form only — no short alias to fat-finger.
        assert!(p(&["sandbox", "setup", "-e"]).is_err());
    }

    #[test]
    fn llama_backends_and_installed() {
        assert_eq!(
            p(&["llama", "backends"]).unwrap(),
            CliCommand::LlamaBackends { build: None }
        );
        assert_eq!(
            p(&["llama", "backends", "--build", "b10883"]).unwrap(),
            CliCommand::LlamaBackends {
                build: Some("b10883".to_string())
            }
        );
        assert_eq!(
            p(&["llama", "backends", "--build=b10883"]).unwrap(),
            CliCommand::LlamaBackends {
                build: Some("b10883".to_string())
            }
        );
        assert_eq!(
            p(&["llama", "installed"]).unwrap(),
            CliCommand::LlamaInstalled
        );
        assert!(p(&["llama"]).is_err()); // no subcommand
        assert!(p(&["llama", "remove"]).is_err()); // unknown subcommand
        assert!(p(&["llama", "backends", "--build"]).is_err()); // no value
        assert!(p(&["llama", "installed", "cpu"]).is_err()); // takes nothing
    }

    #[test]
    fn llama_setup_flags() {
        // The CUDA runtime rides along by default (research §6 F4): without it a
        // `cuda-*` install silently runs on the CPU.
        assert_eq!(
            p(&["llama", "setup", "--backend", "vulkan"]).unwrap(),
            CliCommand::LlamaSetup {
                backend: Some("vulkan".to_string()),
                build: None,
                cudart: true,
                force: false,
                set_binary: false,
            }
        );
        assert_eq!(
            p(&[
                "llama",
                "setup",
                "-b",
                "cuda-12.4",
                "--build",
                "b10883",
                "--no-cudart",
                "--force",
            ])
            .unwrap(),
            CliCommand::LlamaSetup {
                backend: Some("cuda-12.4".to_string()),
                build: Some("b10883".to_string()),
                cudart: false,
                force: true,
                set_binary: false,
            }
        );
        // No default backend (research §6 F3): an omitted `--backend` parses,
        // and the command prints the list instead of choosing a download.
        assert_eq!(
            p(&["llama", "setup"]).unwrap(),
            CliCommand::LlamaSetup {
                backend: None,
                build: None,
                cudart: true,
                force: false,
                set_binary: false,
            }
        );
        // `--set-binary` is long form only, and off unless asked for: the
        // default must never write user data (the `--enable-python` shape).
        assert_eq!(
            p(&["llama", "setup", "-b", "cpu", "--set-binary"]).unwrap(),
            CliCommand::LlamaSetup {
                backend: Some("cpu".to_string()),
                build: None,
                cudart: true,
                force: false,
                set_binary: true,
            }
        );
        assert!(p(&["llama", "setup", "-b", "cpu", "-s"]).is_err());
        assert!(p(&["llama", "setup", "--backend"]).is_err());
        assert!(p(&["llama", "setup", "cpu"]).is_err()); // the backend is an option
        assert!(p(&["llama", "setup", "--cudart"]).is_err());
    }

    #[test]
    fn llama_remove_takes_an_id_and_never_defaults_it() {
        assert_eq!(
            p(&["llama", "remove", "cuda-12.4-b10883"]).unwrap(),
            CliCommand::LlamaRemove {
                id: "cuda-12.4-b10883".to_string(),
                force: false,
            }
        );
        assert_eq!(
            p(&["llama", "remove", "vulkan-b10883", "--force"]).unwrap(),
            CliCommand::LlamaRemove {
                id: "vulkan-b10883".to_string(),
                force: true,
            }
        );
        // Deleting up to a gigabyte with no undo is never implicit.
        assert!(p(&["llama", "remove"]).is_err());
        assert!(p(&["llama", "remove", "a", "b"]).is_err());
        assert!(p(&["llama", "remove", "--all"]).is_err());
    }

    /// Every option of `setup` is one step, in either spelling, and none of
    /// them has a short form: the command writes user data.
    #[test]
    fn setup_parses_every_step_in_both_spellings() {
        assert_eq!(
            p(&[
                "setup",
                "--sandbox",
                "--llama",
                "cuda-12",
                "--llama-build=b11070",
                "--model",
                "/w/chat.gguf",
                "--mmproj=/w/mmproj.gguf",
                "--embed-model",
                "/w/embed.gguf",
                "--ctx",
                "32768",
                "--ngl=-1",
                "--set",
                "engine.managed.sessions=4",
                "--set=engine.external.url=http://h:1/v1?a=b",
                "--verify",
            ])
            .unwrap(),
            CliCommand::Setup(SetupArgs {
                sandbox: true,
                llama: Some("cuda-12".to_string()),
                llama_build: Some("b11070".to_string()),
                model: Some(PathBuf::from("/w/chat.gguf")),
                mmproj: Some(PathBuf::from("/w/mmproj.gguf")),
                embed_model: Some(PathBuf::from("/w/embed.gguf")),
                ctx: Some(32768),
                ngl: Some(-1),
                // Split at the first `=` only: a URL keeps its own.
                set: vec![
                    ("engine.managed.sessions".to_string(), "4".to_string()),
                    (
                        "engine.external.url".to_string(),
                        "http://h:1/v1?a=b".to_string()
                    ),
                ],
                verify: true,
            })
        );
        // One step alone is a use, and bare `setup` parses — the command answers
        // "nothing to do" with its help (the `llama setup` shape).
        let CliCommand::Setup(only_ctx) = p(&["setup", "--ctx", "8192"]).unwrap() else {
            panic!("not a setup")
        };
        assert!(!only_ctx.is_empty() && only_ctx.model.is_none());
        let CliCommand::Setup(bare) = p(&["setup"]).unwrap() else {
            panic!("not a setup")
        };
        assert!(bare.is_empty());
        assert_eq!(
            p(&["setup", "--model", "x", "--help"]).unwrap(),
            CliCommand::Help {
                topic: Some(HelpTopic::Setup)
            }
        );
    }

    #[test]
    fn setup_refuses_what_it_cannot_mean() {
        for bad in [
            &["setup", "--ctx", "lots"][..],
            &["setup", "--ctx", "-1"],
            &["setup", "--ngl", "all"],
            &["setup", "--set", "no-equals-sign"],
            &["setup", "--set", "=value"],
            &["setup", "--set"],
            &["setup", "--model"],
            &["setup", "-m", "x.gguf"], // long forms only
            &["setup", "--enable-python"],
            &["setup", "stray"],
            // A build with no backend to take it from: a forgotten `--llama`.
            &["setup", "--llama-build", "b11070"],
        ] {
            assert!(p(bad).is_err(), "{bad:?}");
        }
        // The two refusals this command owns say what was typed, in both locales.
        for lang in [Lang::En, Lang::Ru] {
            let parse_in = |args: &[&str]| {
                let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
                parse(&owned, locale(lang)).unwrap_err()
            };
            let err = parse_in(&["setup", "--ctx", "lots"]);
            assert!(
                err.contains("--ctx") && err.contains("lots"),
                "{lang:?}: {err}"
            );
            let err = parse_in(&["setup", "--set", "no-equals-sign"]);
            assert!(err.contains("no-equals-sign"), "{lang:?}: {err}");
            assert!(!err.contains('{'), "{lang:?}: {err}");
        }
    }

    /// The topic says the two things someone pasting this into a rented box
    /// needs to hear: a failed step does not stop the rest, and the settings
    /// are written — not overridden.
    #[test]
    fn setup_help_is_listed_and_says_how_it_behaves() {
        let loc = locale(Lang::En);
        assert!(render_help(None, loc).contains("setup  "));
        let topic = render_help(Some(HelpTopic::Setup), loc);
        assert!(topic.contains("mindfork setup [OPTIONS]"), "{topic}");
        for opt in [
            "--sandbox",
            "--llama <BACKEND>",
            "--llama-build <TAG>",
            "--model <GGUF>",
            "--mmproj <GGUF>",
            "--embed-model <GGUF>  ",
            "--ctx <N>",
            "--ngl <N>",
            "--set <KEY>=<VALUE>  ",
            "--verify",
        ] {
            assert!(topic.contains(opt), "{opt}: {topic}");
        }
        assert!(topic.contains("does not stop the others"), "{topic}");
        assert!(topic.contains("settings.json"), "{topic}");
    }

    #[test]
    fn locales_export() {
        assert_eq!(
            p(&["locales", "export", "de", "-o", "de.json"]).unwrap(),
            CliCommand::LocalesExport {
                code: "de".to_string(),
                output: PathBuf::from("de.json")
            }
        );
        assert!(p(&["locales", "export", "de"]).is_err()); // no required --output
        assert!(p(&["locales", "export"]).is_err()); // no code
    }

    #[test]
    fn subcommand_help_topics() {
        assert_eq!(
            p(&["backup", "--help"]).unwrap(),
            CliCommand::Help {
                topic: Some(HelpTopic::Backup)
            }
        );
        assert_eq!(
            p(&["sandbox", "setup", "-h"]).unwrap(),
            CliCommand::Help {
                topic: Some(HelpTopic::SandboxSetup)
            }
        );
        assert_eq!(
            p(&["locales", "export", "--help"]).unwrap(),
            CliCommand::Help {
                topic: Some(HelpTopic::LocalesExport)
            }
        );
        assert_eq!(
            p(&["llama", "--help"]).unwrap(),
            CliCommand::Help {
                topic: Some(HelpTopic::Llama)
            }
        );
        assert_eq!(
            p(&["llama", "setup", "-h"]).unwrap(),
            CliCommand::Help {
                topic: Some(HelpTopic::LlamaSetup)
            }
        );
        assert_eq!(
            p(&["llama", "backends", "-h"]).unwrap(),
            CliCommand::Help {
                topic: Some(HelpTopic::LlamaBackends)
            }
        );
        assert_eq!(
            p(&["llama", "installed", "--help"]).unwrap(),
            CliCommand::Help {
                topic: Some(HelpTopic::LlamaInstalled)
            }
        );
        assert_eq!(
            p(&["llama", "remove", "-h"]).unwrap(),
            CliCommand::Help {
                topic: Some(HelpTopic::LlamaRemove)
            }
        );
    }

    #[test]
    fn unknown_command_and_option() {
        assert!(p(&["nope"]).is_err());
        assert!(p(&["backup", "--nope"]).is_err());
        assert!(p(&["--nope"]).is_err());
    }

    #[test]
    fn missing_option_value() {
        assert!(p(&["backup", "-o"]).is_err()); // option with no value
    }

    #[test]
    fn help_renders_without_unsubstituted_placeholders_for_all_langs() {
        let topics = [
            None,
            Some(HelpTopic::Backup),
            Some(HelpTopic::Restore),
            Some(HelpTopic::Stats),
            Some(HelpTopic::Import),
            Some(HelpTopic::Sandbox),
            Some(HelpTopic::SandboxSetup),
            Some(HelpTopic::Llama),
            Some(HelpTopic::LlamaBackends),
            Some(HelpTopic::LlamaSetup),
            Some(HelpTopic::LlamaInstalled),
            Some(HelpTopic::LlamaRemove),
            Some(HelpTopic::Setup),
            Some(HelpTopic::Demo),
            Some(HelpTopic::Locales),
            Some(HelpTopic::LocalesExport),
        ];
        for &lang in Lang::ALL {
            let loc = locale(lang);
            for topic in topics {
                let text = render_help(topic, loc);
                assert!(!text.contains("{"), "{lang:?} {topic:?}: {text}");
                // Command/flag names are present (not lost during formatting).
                assert!(text.contains("mindfork"), "{lang:?} {topic:?}");
            }
        }
    }

    #[test]
    fn help_columns_leave_gap_after_longest_names() {
        // Regression: the longest command/option name must not run into its
        // description (column width > name length). The gap is
        // language-independent — checked on ru.
        let loc = locale(Lang::Ru);
        // General help: `restore`/`sandbox`/`locales` are the longest command names.
        assert!(
            render_help(None, loc).contains("restore  "),
            "the command name ran into its description"
        );
        // llama setup: `-b, --backend <ID>` shares the column with `--build <TAG>`.
        assert!(
            render_help(Some(HelpTopic::LlamaSetup), loc).contains("--backend <ID>  "),
            "the option name ran into its description"
        );
        // Locale export: `-o, --output <FILE>` is the longest option.
        assert!(
            render_help(Some(HelpTopic::LocalesExport), loc).contains("--output <FILE>  "),
            "the option name ran into its description"
        );
        // Backup/restore: `-p, --password <PASSWORD>` is now the longest option
        // of both — the column had to widen with it.
        for topic in [HelpTopic::Backup, HelpTopic::Restore] {
            assert!(
                render_help(Some(topic), loc).contains("--password <PASSWORD>  "),
                "{topic:?}: the option name ran into its description"
            );
        }
        // Sandbox setup: `--enable-python` is the longest option there.
        assert!(
            render_help(Some(HelpTopic::SandboxSetup), loc).contains("--enable-python  "),
            "the option name ran into its description"
        );
    }
}
