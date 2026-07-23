//! Метаданные приложения для диалога «О программе» (`F1`): автор, ссылки, текст
//! лицензии и список сторонних компонентов с их лицензиями. Слой `shared` (FSD):
//! данные язык-нейтральны (имена/URL/SPDX/юридический текст), поэтому не проходят
//! через локали — их берёт напрямую экран чата (`screens/chat/popups.rs`).
//!
//! Список [`COMPONENTS`] держится в синхроне с прямыми зависимостями `Cargo.toml`
//! гейт-тестом [`tests::components_cover_direct_dependencies`] — как таблица глифа
//! логотипа сверяется с SVG-ассетом (`widgets/logo.rs`). Лицензии выверены по
//! `cargo metadata` (SPDX-идентификаторы крейтов).

/// Бренд-имя приложения (как в логотипе-вордмарке и на crates.io). Пакет/бинарь —
/// `mindfork-rs` (`CARGO_PKG_NAME`), но пользователю показываем короткое «mindfork».
pub const APP_NAME: &str = "mindfork";

/// Автор (совпадает с копирайтом в `LICENSE`).
pub const AUTHOR: &str = "Vladimir Shylov";
/// Сайт проекта (домен застолблён; сайта пока нет).
pub const SITE_URL: &str = "https://mindfork.io";
/// Репозиторий (поле `repository` в `Cargo.toml`).
pub const REPO_URL: &str = "https://github.com/vshylov/mindfork-rs";
/// Страница крейта (имя `mindfork` свободно на crates.io).
pub const CRATE_URL: &str = "https://crates.io/crates/mindfork";

/// Текст лицензии приложения (MIT) — из файла `LICENSE` в корне репозитория.
/// Юридический текст на английском, язык-нейтрален — не локализуется.
pub const LICENSE_TEXT: &str = include_str!("../../LICENSE");

/// Сторонние компоненты — **прямые** зависимости рантайма (`[dependencies]` +
/// `[target.'cfg(windows)'.dependencies]`): `(имя, версия, лицензия-SPDX)`. Dev/
/// build-зависимости (`tempfile`, `winresource`) не входят: они не в поставляемом
/// приложении. Отсортировано по имени. Имена сверяются с `Cargo.toml`, версии — с
/// `Cargo.lock` (гейт-тесты; версия = разрешённая для нашей прямой зависимости).
pub const COMPONENTS: &[(&str, &str, &str)] = &[
    ("ansi-to-tui", "8.0.1", "MIT"),
    ("anyhow", "1.0.103", "MIT OR Apache-2.0"),
    ("arboard", "3.6.1", "MIT OR Apache-2.0"),
    ("async-stream", "0.3.6", "MIT"),
    ("async-trait", "0.1.89", "MIT OR Apache-2.0"),
    ("base64", "0.22.1", "MIT OR Apache-2.0"),
    ("bytemuck", "1.25.0", "Zlib OR Apache-2.0 OR MIT"),
    ("chacha20poly1305", "0.10.1", "Apache-2.0 OR MIT"),
    ("chrono", "0.4.45", "MIT OR Apache-2.0"),
    ("crossterm", "0.29.0", "MIT"),
    ("directories", "6.0.0", "MIT OR Apache-2.0"),
    ("eventsource-stream", "0.2.3", "MIT OR Apache-2.0"),
    ("flate2", "1.1.9", "MIT OR Apache-2.0"),
    ("futures-util", "0.3.32", "MIT OR Apache-2.0"),
    ("hkdf", "0.12.4", "MIT OR Apache-2.0"),
    ("mermaid-text", "0.57.0", "MIT"),
    ("pdf-extract", "0.12.0", "MIT"),
    ("pulldown-cmark", "0.13.4", "MIT"),
    ("quick-xml", "0.39.4", "MIT"),
    ("ratatui", "0.30.1", "MIT"),
    ("reqwest", "0.13.4", "MIT OR Apache-2.0"),
    ("rodio", "0.22.2", "MIT OR Apache-2.0"),
    ("rusqlite", "0.40.1", "MIT"),
    ("scraper", "0.27.0", "ISC"),
    ("serde", "1.0.228", "MIT OR Apache-2.0"),
    ("serde_json", "1.0.150", "MIT OR Apache-2.0"),
    ("sha2", "0.10.9", "MIT OR Apache-2.0"),
    ("single-instance", "0.3.3", "MIT"),
    ("spellbook", "0.4.2", "MPL-2.0"),
    ("sqlite-vec", "0.1.9", "MIT/Apache-2.0"),
    ("syntect", "5.3.0", "MIT"),
    ("sys-locale", "0.3.2", "MIT OR Apache-2.0"),
    ("tar", "0.4.46", "MIT OR Apache-2.0"),
    ("thiserror", "2.0.18", "MIT OR Apache-2.0"),
    ("tokio", "1.52.3", "MIT"),
    ("tokio-util", "0.7.18", "MIT"),
    ("tracing", "0.1.44", "MIT"),
    ("tracing-appender", "0.2.5", "MIT"),
    ("tracing-subscriber", "0.3.23", "MIT"),
    ("tui-scrollview", "0.6.5", "MIT OR Apache-2.0"),
    ("unicode-segmentation", "1.13.3", "MIT OR Apache-2.0"),
    ("unicode-width", "0.2.2", "MIT OR Apache-2.0"),
    ("uuid", "1.23.3", "Apache-2.0 OR MIT"),
    ("windows-sys", "0.61.2", "MIT OR Apache-2.0"),
    ("zip", "2.4.2", "MIT"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// Прямые зависимости рантайма из `Cargo.toml`: секции `[dependencies]` и
    /// `[target.'cfg(windows)'.dependencies]`. Разбор построчный (в этом манифесте
    /// каждая зависимость — одна строка), как парсер SVG в `widgets/logo.rs`.
    fn cargo_runtime_deps() -> BTreeSet<String> {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");
        let toml = std::fs::read_to_string(path).expect("Cargo.toml на месте");
        const WANTED: [&str; 2] = ["[dependencies]", "[target.'cfg(windows)'.dependencies]"];
        let mut section = "";
        let mut deps = BTreeSet::new();
        for line in toml.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('[') {
                section = trimmed;
                continue;
            }
            if !WANTED.contains(&section) || trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if let Some((name, _)) = trimmed.split_once('=') {
                let name = name.trim();
                if !name.is_empty()
                    && name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "-_".contains(c))
                {
                    deps.insert(name.to_string());
                }
            }
        }
        deps
    }

    /// Все версии крейта из `Cargo.lock`: `name → {version, …}` (крейт может иметь
    /// несколько версий — напр. `thiserror` 1/2, `windows-sys` транзитивно).
    fn cargo_lock_versions() -> std::collections::BTreeMap<String, BTreeSet<String>> {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.lock");
        let lock = std::fs::read_to_string(path).expect("Cargo.lock на месте");
        let mut map: std::collections::BTreeMap<String, BTreeSet<String>> = Default::default();
        let mut name: Option<String> = None;
        for line in lock.lines() {
            let t = line.trim();
            if t == "[[package]]" {
                name = None;
            } else if let Some(v) = t.strip_prefix("name = \"") {
                name = v.strip_suffix('"').map(str::to_string);
            } else if let Some(v) = t.strip_prefix("version = \"")
                && let Some(v) = v.strip_suffix('"')
                && let Some(n) = &name
            {
                map.entry(n.clone()).or_default().insert(v.to_string());
            }
        }
        map
    }

    /// Гейт: список компонентов не разошёлся с прямыми зависимостями манифеста.
    /// Добавили/убрали зависимость — правьте [`COMPONENTS`] (и лицензию сверьте по
    /// `cargo metadata`), иначе диалог «О программе» соврёт.
    #[test]
    fn components_cover_direct_dependencies() {
        let manifest = cargo_runtime_deps();
        let listed: BTreeSet<String> = COMPONENTS.iter().map(|(n, ..)| n.to_string()).collect();
        let missing: Vec<_> = manifest.difference(&listed).collect();
        let extra: Vec<_> = listed.difference(&manifest).collect();
        assert!(
            missing.is_empty() && extra.is_empty(),
            "COMPONENTS разошёлся с Cargo.toml — нет в списке: {missing:?}; лишние: {extra:?}"
        );
    }

    /// Гейт: версия каждого компонента присутствует в `Cargo.lock` (устойчиво к
    /// дублям крейта: проверяем вхождение в набор версий). Бамп версии в `Cargo.lock`
    /// уронит тест — обновите версию (и лицензию сверьте) в [`COMPONENTS`].
    #[test]
    fn component_versions_match_cargo_lock() {
        let locked = cargo_lock_versions();
        for (name, version, _) in COMPONENTS {
            let versions = locked
                .get(*name)
                .unwrap_or_else(|| panic!("{name} нет в Cargo.lock"));
            assert!(
                versions.contains(*version),
                "версия {name} {version} не найдена в Cargo.lock: {versions:?}"
            );
        }
    }

    /// Каждая запись несёт непустую лицензию/версию, а список отсортирован по имени
    /// (детерминированный порядок в диалоге).
    #[test]
    fn components_are_sorted_and_licensed() {
        for (name, version, license) in COMPONENTS {
            assert!(!license.is_empty(), "у {name} нет лицензии");
            assert!(!version.is_empty(), "у {name} нет версии");
        }
        let names: Vec<_> = COMPONENTS.iter().map(|(n, ..)| *n).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted, "COMPONENTS не отсортирован по имени");
    }

    /// Текст лицензии встроен и это MIT (а не пустой include).
    #[test]
    fn license_text_is_embedded_mit() {
        assert!(LICENSE_TEXT.contains("MIT License"));
        assert!(LICENSE_TEXT.contains(AUTHOR), "копирайт LICENSE ≠ автору");
    }
}
