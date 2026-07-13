//! Мультиязычность **служебного каркаса агента** (i18n, ось A): промпты фоновых
//! задач, каркас «модели себя», результаты инструментов — тексты, которые читает
//! *модель*. Язык — свойство профиля (`Profile.language`, см. [docs/i18n.md]). Язык
//! **интерфейса** (ось B, тексты для человека) сюда не входит — он отдельное
//! направление и на профили не влияет.
//!
//! Своё микро-решение без крейтов (прецедент `calc.rs`/ADR 0003): `rust-i18n` —
//! глобальная локаль процесса (нам нужна per-profile), `fluent` — оверкилл
//! (плюрализация/грамматика нужны оси B, не промптам). Бандлы — JSON в `locales/`
//! репозитория, **вшиты** в бинарь через `include_str!` (нет режима отказа «файл не
//! найден»; промпты — функциональность, не украшение). Внешние `data/locales/*.json`
//! (override/новые языки без пересборки) — задел Яруса 3.
//!
//! **Формат бандла.** JSON `{"ключ": значение}`, где значение — строка **или массив
//! строк**. Массив склеивается **одним пробелом** (`join(" ")`): длинные промпты в
//! коде — `\`-склеенные однострочники, и разбиение на фрагменты-по-словам в JSON
//! даёт тот же текст (читаемо и без `\n`-эскейпов). Реальный перенос строки — явный
//! `\n` внутри фрагмента.

use std::collections::HashMap;
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

/// Язык служебного каркаса агента (промпты / инструменты / каркас «модели себя»).
/// Значение профиля (`Profile.language`); от языка интерфейса (ось B) независим.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    /// Русский — референсный язык (фолбэк при отсутствии ключа в другом бандле).
    #[default]
    Ru,
    /// English.
    En,
}

/// Референсный язык: если ключа нет в выбранном бандле, берётся отсюда, затем — сам
/// ключ (промпт не должен паниковать из-за опечатки в бандле).
const REFERENCE: Lang = Lang::Ru;

impl Lang {
    /// Все поддерживаемые (вшитые) языки — для перебора в тестах и UI-селекторе.
    pub const ALL: &'static [Lang] = &[Lang::Ru, Lang::En];

    /// Человекочитаемая метка для UI-селектора языка профиля.
    pub fn label(self) -> &'static str {
        match self {
            Lang::Ru => "Русский",
            Lang::En => "English",
        }
    }

    /// Сырой JSON-бандл, вшитый в бинарь (`include_str!` относительно этого файла →
    /// `locales/` в корне репозитория).
    fn bundle_src(self) -> &'static str {
        match self {
            Lang::Ru => include_str!("../../locales/ru.json"),
            Lang::En => include_str!("../../locales/en.json"),
        }
    }
}

/// Загруженный бандл одного языка: плоская таблица «ключ → текст».
pub struct Locale {
    lang: Lang,
    map: HashMap<String, String>,
}

impl Locale {
    /// Язык этой локали — для ключей кэша, зависящих от языка UI (напр. кэш ленты).
    pub fn lang(&self) -> Lang {
        self.lang
    }

    /// Есть ли ключ в бандле (без фолбэка) — для гейт-теста «код ссылается только на
    /// существующие ключи».
    #[cfg(test)]
    pub fn has_key(&self, key: &str) -> bool {
        self.map.contains_key(key)
    }

    /// Значение ключа. Фолбэк: этот язык → референсный (`ru`) → сам ключ. Никогда не
    /// паникует — пропущенный ключ деградирует к референсу/ключу, а не роняет промпт.
    pub fn t<'a>(&'a self, key: &'a str) -> &'a str {
        if let Some(v) = self.map.get(key) {
            return v;
        }
        if self.lang != REFERENCE
            && let Some(v) = locale(REFERENCE).map.get(key)
        {
            return v;
        }
        key
    }

    /// Значение существующего ключа как `&'static` (для динамических ключей вида
    /// `ui.tool.label.{id}`). Требует `&'static self` — оба конца статичны. Без
    /// фолбэка: `None`, если ключа нет (вызывающий подставит русский `&'static`
    /// fallback).
    pub fn get(&'static self, key: &str) -> Option<&'static str> {
        self.map.get(key).map(|s| s.as_str())
    }

    /// Значение с подстановкой именованных плейсхолдеров `{name}`. Простая
    /// текстовая замена (без plural-правил — существующие формулировки нейтральны к
    /// числу: «×{n}», «заметок: {n}»).
    pub fn tf(&self, key: &str, args: &[(&str, &str)]) -> String {
        let mut s = self.t(key).to_string();
        for (k, v) in args {
            s = s.replace(&format!("{{{k}}}"), v);
        }
        s
    }
}

/// Разбирает вшитый JSON-бандл в плоскую таблицу. Значение-массив склеивается одним
/// пробелом (см. док модуля). Паника только на битом бандле — вшит и покрыт
/// gate-тестом полноты, поэтому в рантайме недостижима.
fn parse_bundle(lang: Lang) -> Locale {
    let raw: HashMap<String, serde_json::Value> = serde_json::from_str(lang.bundle_src())
        .unwrap_or_else(|e| panic!("бандл локали {lang:?}: ошибка разбора JSON: {e}"));
    let map = raw
        .into_iter()
        .map(|(k, v)| {
            let text = match v {
                serde_json::Value::String(s) => s,
                serde_json::Value::Array(a) => a
                    .iter()
                    .map(|x| {
                        x.as_str().unwrap_or_else(|| {
                            panic!("ключ {k} ({lang:?}): элемент массива не строка")
                        })
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
                other => {
                    panic!("ключ {k} ({lang:?}): ожидалась строка или массив, получено {other}")
                }
            };
            (k, text)
        })
        .collect();
    Locale { lang, map }
}

static RU: LazyLock<Locale> = LazyLock::new(|| parse_bundle(Lang::Ru));
static EN: LazyLock<Locale> = LazyLock::new(|| parse_bundle(Lang::En));

/// Возвращает вшитый бандл языка (`&'static` — удобно класть в снимки хода/задач).
pub fn locale(lang: Lang) -> &'static Locale {
    match lang {
        Lang::Ru => &RU,
        Lang::En => &EN,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_bundles_parse() {
        // Разбор каждого вшитого бандла (ловит битый JSON/неверные типы значений).
        for &lang in Lang::ALL {
            let _ = locale(lang);
        }
    }

    #[test]
    fn key_sets_match_across_languages() {
        // Множества ключей всех бандлов совпадают — перевод не отстаёт и не забегает.
        let ref_keys: std::collections::BTreeSet<&String> = locale(REFERENCE).map.keys().collect();
        for &lang in Lang::ALL {
            if lang == REFERENCE {
                continue;
            }
            let keys: std::collections::BTreeSet<&String> = locale(lang).map.keys().collect();
            let missing: Vec<_> = ref_keys.difference(&keys).collect();
            let extra: Vec<_> = keys.difference(&ref_keys).collect();
            assert!(
                missing.is_empty() && extra.is_empty(),
                "{lang:?}: отсутствуют {missing:?}, лишние {extra:?}"
            );
        }
    }

    #[test]
    fn placeholder_sets_match_across_languages() {
        // Набор плейсхолдеров {…} каждого ключа одинаков во всех языках — иначе
        // подстановка `tf` оставит дыру или проигнорирует аргумент.
        fn placeholders(s: &str) -> std::collections::BTreeSet<String> {
            let mut out = std::collections::BTreeSet::new();
            let mut rest = s;
            while let Some(i) = rest.find('{') {
                if let Some(j) = rest[i..].find('}') {
                    out.insert(rest[i + 1..i + j].to_string());
                    rest = &rest[i + j + 1..];
                } else {
                    break;
                }
            }
            out
        }
        for (key, val) in &locale(REFERENCE).map {
            let want = placeholders(val);
            for &lang in Lang::ALL {
                if lang == REFERENCE {
                    continue;
                }
                let got = placeholders(locale(lang).t(key));
                assert_eq!(want, got, "ключ {key}: плейсхолдеры {lang:?} расходятся");
            }
        }
    }

    #[test]
    fn en_bundle_has_no_cyrillic() {
        // Прямой прокси go-критерия (docs/i18n.md, Ярус 1): en-каркас не содержит
        // кириллицы. Ловит случайно оставленный русский текст в переводе. tool-имена
        // и ключи — ASCII, так что чистый en-бандл сплошь латиница/пунктуация.
        for (key, val) in &locale(Lang::En).map {
            let cyr = val
                .chars()
                .find(|c| ('а'..='я').contains(c) || ('А'..='Я').contains(c));
            assert!(
                cyr.is_none(),
                "ключ {key}: кириллица в en-переводе: {val:?}"
            );
        }
    }

    #[test]
    fn fallback_to_reference_then_key() {
        let en = locale(Lang::En);
        // Несуществующий ключ → сам ключ (не паника).
        assert_eq!(en.t("no.such.key.exists"), "no.such.key.exists");
    }

    #[test]
    fn tf_substitutes_named_placeholders() {
        // На реальном ключе с плейсхолдером (возраст «N дн.»).
        for &lang in Lang::ALL {
            let s = locale(lang).tf("selfmodel.age.days", &[("n", "3")]);
            assert!(s.contains('3') && !s.contains("{n}"), "{lang:?}: {s}");
        }
    }

    #[test]
    fn all_ui_keys_referenced_in_code_exist_in_bundle() {
        // Гейт против класса бага «код зовёт loc.t("ui.…"), а ключа нет в бандле»
        // (тогда `t` молча возвращает сам ключ — в UI виден слаг вместо текста).
        // Сканируем исходники на литералы `ui.*`-ключей и проверяем, что каждый есть
        // в референсном (`ru`) бандле. Динамические ключи (собираемые `format!`)
        // сюда не попадут — их немного и они покрыты render-тестами.
        use std::path::Path;
        // Литерал ключа: "ui." + сегменты из [a-z0-9_] через точки.
        let key_re = |s: &str| -> Vec<String> {
            let mut out = Vec::new();
            let bytes = s.as_bytes();
            let mut i = 0;
            while let Some(p) = s[i..].find("\"ui.") {
                let start = i + p + 1; // после кавычки
                let mut j = start;
                while j < bytes.len() {
                    let c = bytes[j] as char;
                    if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.' {
                        j += 1;
                    } else {
                        break;
                    }
                }
                // Ключ валиден, если следом идёт закрывающая кавычка (литерал целиком).
                if j < bytes.len() && bytes[j] as char == '"' {
                    out.push(s[start..j].to_string());
                }
                i = j;
            }
            out
        };
        fn visit(dir: &Path, keys: &mut Vec<String>, key_re: &dyn Fn(&str) -> Vec<String>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    visit(&path, keys, key_re);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let src = std::fs::read_to_string(&path).unwrap_or_default();
                    keys.extend(key_re(&src));
                }
            }
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut keys = Vec::new();
        visit(&root, &mut keys, &key_re);
        // Отсекаем вырожденные (напр. «ui.» из собственного regex-литерала этого теста).
        keys.retain(|k| k.len() > 3 && !k.ends_with('.'));
        keys.sort();
        keys.dedup();
        let ru = locale(Lang::Ru);
        let missing: Vec<&String> = keys.iter().filter(|k| !ru.has_key(k)).collect();
        assert!(
            missing.is_empty(),
            "ключи `ui.*` есть в коде, но отсутствуют в бандле: {missing:?}"
        );
    }
}
