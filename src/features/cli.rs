//! Собственный микро-парсер аргументов командной строки. Заменяет `clap`, чтобы
//! **весь** текст CLI (справка, ошибки разбора) жил в бандлах локалей (i18n, ось B —
//! docs/history/i18n-cli.md): `clap` не даёт локализовать сообщения об ошибках разбора, а его
//! derive-атрибуты не принимают параметр-локаль (пришлось бы к глобальной локали
//! процесса — ровно то, за что отвергли `rust-i18n`). Поверхность мала (5 подкоманд,
//! несколько опций), поэтому свой парсер — по прецеденту markdown (ADR 0003), i18n,
//! `calc`, InputBox (ADR 0001): своё решение, когда крейт мешает требованию.
//!
//! Локаль передаётся **параметром** (без глобалов): [`parse`] и [`render_help`]
//! берут `&Locale`, все тексты — из бандла. `Err(String)` из [`parse`] — уже готовое
//! к печати сообщение (префикс + причина + подсказка про `--help`).
//!
//! **Не переводятся** имена подкоманд/флагов (`backup`, `--output`) — это протокол,
//! как id инструментов и команды `/rag` (docs/history/i18n.md §2.3).

use std::path::PathBuf;

use crate::shared::i18n::Locale;

/// Степень сжатия бэкапа по умолчанию (совпадает с прежним `default_value_t = 9`).
pub const DEFAULT_COMPRESSION: i64 = 9;

/// Разобранная команда CLI. Без подкоманды → [`CliCommand::Run`] (запуск TUI).
#[derive(Debug, PartialEq, Eq)]
pub enum CliCommand {
    /// Запустить TUI (аргументов не было).
    Run,
    /// Создать резервную копию (`backup`).
    Backup {
        output: Option<PathBuf>,
        compression: i64,
    },
    /// Восстановить из резервной копии (`restore <archive>`).
    Restore { archive: PathBuf },
    /// Импорт из файла формата mindfork-import (`import <file>`).
    /// Спецификация формата — docs/import-format.md.
    Import { file: PathBuf },
    /// Установка песочницы Python (`sandbox setup [--force]`).
    SandboxSetup { force: bool },
    /// Установка локального озвучивания (`tts setup [--force] [--voice NAME]`).
    TtsSetup { force: bool, voices: Vec<String> },
    /// Экспорт бандла локали (`locales export <code> --output <file>`).
    LocalesExport { code: String, output: PathBuf },
    /// Показать справку (общую или по подкоманде).
    Help { topic: Option<HelpTopic> },
    /// Показать версию (`-V`/`--version`).
    Version,
}

/// Тема справки: общая (`None` у [`CliCommand::Help`]) либо конкретная подкоманда.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum HelpTopic {
    Backup,
    Restore,
    Import,
    Sandbox,
    SandboxSetup,
    Tts,
    TtsSetup,
    Locales,
    LocalesExport,
}

/// Разбирает аргументы (после имени программы). `Err` — уже готовое к печати
/// локализованное сообщение (префикс + причина + подсказка про `--help`).
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
        // Команда удалена (этап 1 направления «плагины»): импорт LameLLaMA теперь
        // выполняет внешний конвертер, эмитящий файл mindfork-import. Подсказываем
        // замену вместо генерического «неизвестная команда».
        "import-lamellama" => Err(err_line(loc, "cli.import.lamellama_removed", &[])),
        "sandbox" => parse_sandbox(rest, loc),
        "tts" => parse_tts(rest, loc),
        "locales" => parse_locales(rest, loc),
        other if other.starts_with('-') => Err(unknown_option(loc, other)),
        other => Err(unknown_command(loc, other)),
    }
}

fn parse_backup(toks: &[&str], loc: &Locale) -> Result<CliCommand, String> {
    let mut output = None;
    let mut compression = DEFAULT_COMPRESSION;
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
        } else if a.starts_with('-') {
            return Err(unknown_option(loc, a));
        } else {
            return Err(unexpected_arg(loc, a));
        }
    }
    Ok(CliCommand::Backup {
        output,
        compression,
    })
}

fn parse_restore(toks: &[&str], loc: &Locale) -> Result<CliCommand, String> {
    match single_positional(toks, loc, HelpTopic::Restore, "<archive>")? {
        Positional::Help => Ok(CliCommand::Help {
            topic: Some(HelpTopic::Restore),
        }),
        Positional::Value(archive) => Ok(CliCommand::Restore {
            archive: PathBuf::from(archive),
        }),
    }
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
    for &a in toks {
        match a {
            "-h" | "--help" => {
                return Ok(CliCommand::Help {
                    topic: Some(HelpTopic::SandboxSetup),
                });
            }
            "-f" | "--force" => force = true,
            _ if a.starts_with('-') => return Err(unknown_option(loc, a)),
            _ => return Err(unexpected_arg(loc, a)),
        }
    }
    Ok(CliCommand::SandboxSetup { force })
}

fn parse_tts(toks: &[&str], loc: &Locale) -> Result<CliCommand, String> {
    let Some((&sub, rest)) = toks.split_first() else {
        return Err(missing_subcommand(loc, "tts"));
    };
    match sub {
        "-h" | "--help" => Ok(CliCommand::Help {
            topic: Some(HelpTopic::Tts),
        }),
        "setup" => parse_tts_setup(rest, loc),
        other if other.starts_with('-') => Err(unknown_option(loc, other)),
        other => Err(unknown_subcommand(loc, other, "tts")),
    }
}

fn parse_tts_setup(toks: &[&str], loc: &Locale) -> Result<CliCommand, String> {
    let mut force = false;
    let mut voices = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let a = toks[i];
        if a == "-h" || a == "--help" {
            return Ok(CliCommand::Help {
                topic: Some(HelpTopic::TtsSetup),
            });
        } else if a == "-f" || a == "--force" {
            force = true;
            i += 1;
        } else if let Some(v) = opt_value(toks, &mut i, loc, &["-v", "--voice"])? {
            voices.push(v.to_string());
        } else if a.starts_with('-') {
            return Err(unknown_option(loc, a));
        } else {
            return Err(unexpected_arg(loc, a));
        }
    }
    Ok(CliCommand::TtsSetup { force, voices })
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

// -------- Разбор одиночного обязательного позиционного аргумента (restore/import) --------

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

// -------- Опция со значением: `--opt value` и `--opt=value` (обе формы + короткая) --------

/// Если `toks[*i]` совпадает с одним из `aliases`, извлекает значение (следующий
/// токен для формы `--opt value`, либо часть после `=` для `--opt=value`), продвигает
/// `*i` и возвращает `Ok(Some(value))`. Если токен не совпал ни с одним алиасом —
/// `Ok(None)` (индекс не двигается). Совпал, но значения нет — `Err`.
fn opt_value<'a>(
    toks: &[&'a str],
    i: &mut usize,
    loc: &Locale,
    aliases: &[&str],
) -> Result<Option<&'a str>, String> {
    let a = toks[*i];
    // Форма `--opt value`: точное совпадение имени, значение — следующий токен.
    if aliases.contains(&a) {
        let display = aliases.last().copied().unwrap_or(a);
        let v = *toks
            .get(*i + 1)
            .ok_or_else(|| missing_value(loc, display))?;
        *i += 2;
        return Ok(Some(v));
    }
    // Форма `--opt=value` / `-o=value`.
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

// -------- Конструкторы сообщений об ошибках (готовый к печати текст) --------

/// Полная строка ошибки: `{префикс}: {причина}\n\n{подсказка про --help}`.
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

// -------- Справка --------

/// Текст справки: общий (`topic == None`) либо по подкоманде. Все подписи — из бандла;
/// имена команд/флагов и метки аргументов (`<archive>`) — протокол, не переводятся.
pub fn render_help(topic: Option<HelpTopic>, loc: &Locale) -> String {
    let usage = loc.t("cli.help.usage");
    let commands = loc.t("cli.help.commands");
    let options = loc.t("cli.help.options");
    let arguments = loc.t("cli.help.arguments");
    match topic {
        None => format!(
            "{about}\n\n{usage} mindfork-rs [COMMAND]\n\n{commands}\n\
             {b:<20}{cb}\n{r:<20}{cr}\n{im:<20}{ci}\n{sb:<20}{cs}\n{tt:<20}{ct}\n{lc:<20}{cl}\n\n\
             {options}\n  -h, --help     {oh}\n  -V, --version  {ov}",
            about = loc.t("cli.help.about"),
            b = "  backup",
            cb = loc.t("cli.help.cmd.backup"),
            r = "  restore",
            cr = loc.t("cli.help.cmd.restore"),
            im = "  import",
            ci = loc.t("cli.help.cmd.import"),
            sb = "  sandbox",
            cs = loc.t("cli.help.cmd.sandbox"),
            tt = "  tts",
            ct = loc.t("cli.help.cmd.tts"),
            lc = "  locales",
            cl = loc.t("cli.help.cmd.locales"),
            oh = loc.t("cli.help.opt.help"),
            ov = loc.t("cli.help.opt.version"),
        ),
        Some(HelpTopic::Backup) => format!(
            "{d}\n\n{usage} mindfork-rs backup [OPTIONS]\n\n{options}\n\
             {o:<28}{co}\n{c:<28}{cc}\n{h:<28}{ch}",
            d = loc.t("cli.help.cmd.backup"),
            o = "  -o, --output <FILE>",
            co = loc.t("cli.help.opt.backup.output"),
            c = "  -c, --compression <0-9>",
            cc = loc.t("cli.help.opt.backup.compression"),
            h = "  -h, --help",
            ch = loc.t("cli.help.opt.help"),
        ),
        Some(HelpTopic::Restore) => format!(
            "{d}\n\n{usage} mindfork-rs restore <ARCHIVE>\n\n{arguments}\n\
             {a:<16}{ca}",
            d = loc.t("cli.help.cmd.restore"),
            a = "  <ARCHIVE>",
            ca = loc.t("cli.help.arg.restore.archive"),
        ),
        Some(HelpTopic::Import) => format!(
            "{d}\n\n{usage} mindfork-rs import <FILE>\n\n{arguments}\n\
             {a:<12}{ca}",
            d = loc.t("cli.help.cmd.import"),
            a = "  <FILE>",
            ca = loc.t("cli.help.arg.import.file"),
        ),
        Some(HelpTopic::Sandbox) => format!(
            "{d}\n\n{usage} mindfork-rs sandbox <COMMAND>\n\n{commands}\n{s:<12}{cs}",
            d = loc.t("cli.help.cmd.sandbox"),
            s = "  setup",
            cs = loc.t("cli.help.cmd.sandbox.setup"),
        ),
        Some(HelpTopic::SandboxSetup) => format!(
            "{d}\n\n{usage} mindfork-rs sandbox setup [OPTIONS]\n\n{options}\n\
             {f:<18}{cf}\n{h:<18}{ch}",
            d = loc.t("cli.help.cmd.sandbox.setup"),
            f = "  -f, --force",
            cf = loc.t("cli.help.opt.sandbox.force"),
            h = "  -h, --help",
            ch = loc.t("cli.help.opt.help"),
        ),
        Some(HelpTopic::Tts) => format!(
            "{d}

{usage} mindfork-rs tts <COMMAND>

{commands}
{s:<12}{cs}",
            d = loc.t("cli.help.cmd.tts"),
            s = "  setup",
            cs = loc.t("cli.help.cmd.tts.setup"),
        ),
        Some(HelpTopic::TtsSetup) => format!(
            "{d}

{usage} mindfork-rs tts setup [OPTIONS]

{options}
             {f:<24}{cf}
{v:<24}{cv}
{h:<24}{ch}",
            d = loc.t("cli.help.cmd.tts.setup"),
            f = "  -f, --force",
            cf = loc.t("cli.help.opt.tts.force"),
            v = "  -v, --voice <NAME>",
            cv = loc.t("cli.help.opt.tts.voice"),
            h = "  -h, --help",
            ch = loc.t("cli.help.opt.help"),
        ),
        Some(HelpTopic::Locales) => format!(
            "{d}\n\n{usage} mindfork-rs locales <COMMAND>\n\n{commands}\n{e:<12}{ce}",
            d = loc.t("cli.help.cmd.locales"),
            e = "  export",
            ce = loc.t("cli.help.cmd.locales.export"),
        ),
        Some(HelpTopic::LocalesExport) => format!(
            "{d}\n\n{usage} mindfork-rs locales export <CODE> --output <FILE>\n\n\
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
                compression: DEFAULT_COMPRESSION
            }
        );
        // Обе формы опций: `--opt value` и `--opt=value`, короткая `-o`.
        assert_eq!(
            p(&["backup", "-o", "a.zip", "-c", "3"]).unwrap(),
            CliCommand::Backup {
                output: Some(PathBuf::from("a.zip")),
                compression: 3
            }
        );
        assert_eq!(
            p(&["backup", "--output=b.zip", "--compression=0"]).unwrap(),
            CliCommand::Backup {
                output: Some(PathBuf::from("b.zip")),
                compression: 0
            }
        );
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
                archive: PathBuf::from("a.zip")
            }
        );
        assert!(p(&["restore"]).is_err()); // нет обязательного аргумента
        assert!(p(&["restore", "a.zip", "b.zip"]).is_err()); // лишний аргумент
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
        // Удалённая команда даёт подсказку про внешний конвертер + `import`,
        // а не генерическое «неизвестная команда».
        let err = p(&["import-lamellama", "d"]).unwrap_err();
        assert!(err.contains("import"), "{err}");
        assert!(err.contains("mindfork-import"), "{err}");
    }

    #[test]
    fn sandbox_setup_force() {
        assert_eq!(
            p(&["sandbox", "setup"]).unwrap(),
            CliCommand::SandboxSetup { force: false }
        );
        assert_eq!(
            p(&["sandbox", "setup", "--force"]).unwrap(),
            CliCommand::SandboxSetup { force: true }
        );
        assert!(p(&["sandbox"]).is_err()); // нет подкоманды
        assert!(p(&["sandbox", "teardown"]).is_err()); // неизвестная подкоманда
    }

    #[test]
    fn tts_setup_force_and_voices() {
        assert_eq!(
            p(&["tts", "setup"]).unwrap(),
            CliCommand::TtsSetup {
                force: false,
                voices: vec![]
            }
        );
        // `--voice` можно повторять (обе формы записи значения).
        assert_eq!(
            p(&[
                "tts",
                "setup",
                "-f",
                "--voice",
                "ru_RU-denis-medium",
                "--voice=en_GB-alan-medium"
            ])
            .unwrap(),
            CliCommand::TtsSetup {
                force: true,
                voices: vec![
                    "ru_RU-denis-medium".to_string(),
                    "en_GB-alan-medium".to_string()
                ]
            }
        );
        assert!(p(&["tts"]).is_err()); // нет подкоманды
        assert!(p(&["tts", "remove"]).is_err()); // неизвестная подкоманда
        assert!(p(&["tts", "setup", "--voice"]).is_err()); // нет значения опции
        assert!(p(&["tts", "setup", "лишнее"]).is_err()); // позиционных нет
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
        assert!(p(&["locales", "export", "de"]).is_err()); // нет обязательного --output
        assert!(p(&["locales", "export"]).is_err()); // нет кода
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
    }

    #[test]
    fn unknown_command_and_option() {
        assert!(p(&["nope"]).is_err());
        assert!(p(&["backup", "--nope"]).is_err());
        assert!(p(&["--nope"]).is_err());
    }

    #[test]
    fn missing_option_value() {
        assert!(p(&["backup", "-o"]).is_err()); // опция без значения
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
            Some(HelpTopic::Locales),
            Some(HelpTopic::LocalesExport),
        ];
        for &lang in Lang::ALL {
            let loc = locale(lang);
            for topic in topics {
                let text = render_help(topic, loc);
                assert!(!text.contains("{"), "{lang:?} {topic:?}: {text}");
                // Имена команд/флагов присутствуют (не потерялись при форматировании).
                assert!(text.contains("mindfork-rs"), "{lang:?} {topic:?}");
            }
        }
    }

    #[test]
    fn help_columns_leave_gap_after_longest_names() {
        // Регрессия: самое длинное имя команды/опции не должно слипаться с описанием
        // (ширина колонки > длины имени). Гэп языко-независим — проверяем на ru.
        let loc = locale(Lang::Ru);
        // Общая справка: `restore`/`sandbox`/`locales` — самые длинные имена команд.
        assert!(
            render_help(None, loc).contains("restore  "),
            "имя команды слиплось с описанием"
        );
        // Экспорт локали: `-o, --output <FILE>` — самая длинная опция.
        assert!(
            render_help(Some(HelpTopic::LocalesExport), loc).contains("--output <FILE>  "),
            "имя опции слиплось с описанием"
        );
    }
}
