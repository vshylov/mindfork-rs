//! Our own micro command-line argument parser. Replaces `clap` so that
//! **all** CLI text (help, parse errors) lives in the locale bundles (i18n,
//! axis B — docs/history/i18n-cli.md): `clap` doesn't let us localize parse-error
//! messages, and its derive attributes don't accept a locale parameter (we'd
//! have had to fall back to a global process locale — exactly what we
//! rejected `rust-i18n` for). The surface is small (5 subcommands, a handful
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
    },
    /// List the llama.cpp builds already in `data/llama/` (`llama installed`).
    LlamaInstalled,
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

/// Help topic: general (`None` for [`CliCommand::Help`]) or a specific subcommand.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum HelpTopic {
    Backup,
    Restore,
    Import,
    Sandbox,
    SandboxSetup,
    Llama,
    LlamaBackends,
    LlamaSetup,
    LlamaInstalled,
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
        "import" => parse_import(rest, loc),
        // The command was removed (stage 1 of the "plugins" track): LameLLaMA
        // import is now done by an external converter that emits a
        // mindfork-import file. We hint at the replacement instead of a
        // generic "unknown command".
        "import-lamellama" => Err(err_line(loc, "cli.import.lamellama_removed", &[])),
        "sandbox" => parse_sandbox(rest, loc),
        "llama" => parse_llama(rest, loc),
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

fn parse_restore(toks: &[&str], loc: &Locale) -> Result<CliCommand, String> {
    let mut archive: Option<&str> = None;
    let mut password = None;
    let mut i = 0;
    while i < toks.len() {
        let a = toks[i];
        if a == "-h" || a == "--help" {
            return Ok(CliCommand::Help {
                topic: Some(HelpTopic::Restore),
            });
        } else if let Some(v) = opt_value(toks, &mut i, loc, &["-p", "--password"])? {
            password = Some(v.to_string());
        } else if a.starts_with('-') {
            return Err(unknown_option(loc, a));
        } else if archive.is_none() {
            archive = Some(a);
            i += 1;
        } else {
            return Err(unexpected_arg(loc, a));
        }
    }
    Ok(CliCommand::Restore {
        archive: PathBuf::from(archive.ok_or_else(|| missing_arg(loc, "<archive>"))?),
        password,
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
             {dm:<20}{cd}\n{b:<20}{cb}\n{r:<20}{cr}\n{im:<20}{ci}\n{sb:<20}{cs}\n{ll:<20}{cll}\n{lc:<20}{cl}\n\n\
             {options}\n  -h, --help     {oh}\n  -V, --version  {ov}",
            about = loc.t("cli.help.about"),
            dm = "  demo",
            cd = loc.t("cli.help.cmd.demo"),
            b = "  backup",
            cb = loc.t("cli.help.cmd.backup"),
            r = "  restore",
            cr = loc.t("cli.help.cmd.restore"),
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
             {b:<14}{cb}\n{s:<14}{cs}\n{i:<14}{ci}",
            d = loc.t("cli.help.cmd.llama"),
            b = "  backends",
            cb = loc.t("cli.help.cmd.llama.backends"),
            s = "  setup",
            cs = loc.t("cli.help.cmd.llama.setup"),
            i = "  installed",
            ci = loc.t("cli.help.cmd.llama.installed"),
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
             {b:<24}{cb}\n{bu:<24}{cbu}\n{n:<24}{cn}\n{f:<24}{cf}\n{h:<24}{ch}",
            d = loc.t("cli.help.cmd.llama.setup"),
            b = "  -b, --backend <ID>",
            cb = loc.t("cli.help.opt.llama.backend"),
            bu = "  --build <TAG>",
            cbu = loc.t("cli.help.opt.llama.build"),
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
            }
        );
        assert!(p(&["llama", "setup", "--backend"]).is_err());
        assert!(p(&["llama", "setup", "cpu"]).is_err()); // the backend is an option
        assert!(p(&["llama", "setup", "--cudart"]).is_err());
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
            Some(HelpTopic::Import),
            Some(HelpTopic::Sandbox),
            Some(HelpTopic::SandboxSetup),
            Some(HelpTopic::Llama),
            Some(HelpTopic::LlamaBackends),
            Some(HelpTopic::LlamaSetup),
            Some(HelpTopic::LlamaInstalled),
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
