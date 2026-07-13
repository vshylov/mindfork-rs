//! Мультиязычность **служебного каркаса агента** (i18n, ось A): промпты фоновых
//! задач, каркас «модели себя», результаты инструментов — тексты, которые читает
//! *модель*. Язык — свойство профиля (`Profile.language`, см. [docs/i18n.md]). Язык
//! **интерфейса** (ось B, тексты для человека) использует тот же механизм через
//! ключи `ui.*` и хранится глобально (`config.interface.language`), на профили не
//! влияя.
//!
//! Своё микро-решение без крейтов (прецедент `calc.rs`/ADR 0003): `rust-i18n` —
//! глобальная локаль процесса (нам нужна per-profile), `fluent` — оверкилл
//! (плюрализация/грамматика нужны оси B, не промптам). Бандлы — JSON в `locales/`
//! репозитория, **вшиты** в бинарь через `include_str!` (нет режима отказа «файл не
//! найден»; промпты — функциональность, не украшение).
//!
//! **Внешние локали (Ярус 3, [docs/i18n-external-locales.md]).** При старте
//! [`init`] сканирует `data/locales/*.json`: файл `<code>.json` мержится **поверх**
//! вшитого бандла того же кода (частичный override — переопределяются только
//! присутствующие ключи), а файл с новым кодом добавляет **новый язык** ([`Lang::Ext`])
//! без пересборки (недостающие ключи → фолбэк к референсу `ru`). Битый/нечитаемый
//! внешний файл — предупреждение в лог + пропуск (мягкая деградация: вшитое цело).
//! Без `init` (тесты) — только вшитые бандлы, поведение неизменно.
//!
//! **Формат бандла.** JSON `{"ключ": значение}`, где значение — строка **или массив
//! строк**. Массив склеивается **одним пробелом** (`join(" ")`): длинные промпты в
//! коде — `\`-склеенные однострочники, и разбиение на фрагменты-по-словам в JSON
//! даёт тот же текст (читаемо и без `\n`-эскейпов). Реальный перенос строки — явный
//! `\n` внутри фрагмента.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{LazyLock, Mutex, OnceLock};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Язык служебного каркаса агента (промпты / инструменты / каркас «модели себя»).
/// Значение профиля (`Profile.language`) и языка интерфейса (`config.interface.language`).
///
/// Вшитые `Ru`/`En` — отдельные варианты (используются как `Lang::Ru`/`Lang::En`);
/// `Ext(code)` — язык из внешнего файла `data/locales/<code>.json`. Код
/// интернирован (`&'static str`, `Box::leak`), поэтому тип остаётся `Copy`; `Eq`/`Hash`
/// сравнивают по содержимому кода (не по указателю).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Lang {
    /// Русский — референсный язык (фолбэк при отсутствии ключа в другом бандле).
    #[default]
    Ru,
    /// English.
    En,
    /// Внешний язык из `data/locales/<code>.json` (код — интернированная строка).
    Ext(&'static str),
}

/// Референсный язык: если ключа нет в выбранном бандле, берётся отсюда, затем — сам
/// ключ (промпт не должен паниковать из-за опечатки в бандле).
const REFERENCE: Lang = Lang::Ru;

/// Интернер кодов внешних языков: рантайм-`String` → `&'static str` (`Box::leak`).
/// Языков конечно — ограниченный однократный леак (прецедент — кэш syntect-тем
/// `markdown/code.rs`). Вшитые `ru`/`en` — литералы, интернер их не касается.
static INTERN: LazyLock<Mutex<HashSet<&'static str>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

fn intern(code: &str) -> &'static str {
    let mut set = INTERN.lock().expect("интернер кодов языков отравлен");
    if let Some(s) = set.get(code) {
        return s;
    }
    let leaked: &'static str = Box::leak(code.to_string().into_boxed_str());
    set.insert(leaked);
    leaked
}

impl Lang {
    /// Вшитые языки — для гейт-тестов полноты и per-locale структурных тестов (они
    /// проверяют **вшитые** бандлы; внешние — пользовательский контент, не гейтятся).
    /// UI-селекторы и функциональные потребители используют [`Lang::all`] (реестр).
    pub const ALL: &'static [Lang] = &[Lang::Ru, Lang::En];

    /// Стабильный код языка (для serde и имени файла локали).
    pub fn code(self) -> &'static str {
        match self {
            Lang::Ru => "ru",
            Lang::En => "en",
            Lang::Ext(c) => c,
        }
    }

    /// Язык по коду: вшитые `ru`/`en` → соответствующий вариант, иначе — внешний
    /// [`Lang::Ext`] с интернированным кодом. Интернирование не зависит от реестра,
    /// поэтому профиль с неизвестным кодом десериализуется и до вызова [`init`].
    pub fn from_code(code: &str) -> Lang {
        match code {
            "ru" => Lang::Ru,
            "en" => Lang::En,
            other => Lang::Ext(intern(other)),
        }
    }

    /// Все языки, известные приложению: вшитые + найденные внешние (детерминированный
    /// порядок: `Ru`, `En`, затем `Ext` по коду). В тестах (без [`init`]) = `[Ru, En]`.
    /// Для UI-селекторов и функциональных потребителей (напр. распознавание метки
    /// консоли по всем языкам).
    pub fn all() -> Vec<Lang> {
        all_from(registry())
    }

    /// Человекочитаемая метка для UI-селектора языка: имя языка в его собственном
    /// написании из **своего** бандла (ключ `ui.lang.name`). Для внешнего языка без
    /// этого ключа — его код; вшитые ru/en имеют жёсткий фолбэк на случай, если ключ
    /// вдруг переопределён пустым. Читаем именно свой бандл ([`locale_exact`], без
    /// фолбэка к референсу) — иначе внешний язык без `ui.lang.name` показал бы имя ru.
    pub fn label(self) -> &'static str {
        let fallback = match self {
            Lang::Ru => "Русский",
            Lang::En => "English",
            Lang::Ext(code) => code,
        };
        locale_exact(self)
            .and_then(|l| l.get("ui.lang.name"))
            .unwrap_or(fallback)
    }

    /// Сырой JSON-бандл, вшитый в бинарь (`include_str!` относительно этого файла →
    /// `locales/` в корне репозитория). Только для вшитых вариантов.
    fn bundle_src(self) -> &'static str {
        match self {
            Lang::Ru => include_str!("../../locales/ru.json"),
            Lang::En => include_str!("../../locales/en.json"),
            Lang::Ext(_) => unreachable!("внешний язык не имеет вшитого источника"),
        }
    }
}

impl Serialize for Lang {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.code())
    }
}

impl<'de> Deserialize<'de> for Lang {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let code = String::deserialize(d)?;
        Ok(Lang::from_code(&code))
    }
}

/// Языки реестра в детерминированном порядке (выделено для теста над локальной картой,
/// без глобального реестра).
fn all_from(reg: &HashMap<Lang, &'static Locale>) -> Vec<Lang> {
    let mut v: Vec<Lang> = reg.keys().copied().collect();
    sort_langs(&mut v);
    v
}

/// Детерминированный порядок языков в UI: `Ru`, `En`, затем внешние по коду.
fn sort_langs(v: &mut [Lang]) {
    v.sort_by_key(|l| match l {
        Lang::Ru => (0u8, ""),
        Lang::En => (1u8, ""),
        Lang::Ext(c) => (2u8, *c),
    });
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

/// Разбирает JSON-бандл в плоскую таблицу «ключ → текст». Значение-массив
/// склеивается одним пробелом (см. док модуля). Возвращает `Err` (не паникует) —
/// подходит и вшитым (обёрнуто паникой), и внешним файлам (обёрнуто предупреждением).
fn json_to_map(src: &str) -> Result<HashMap<String, String>, String> {
    let raw: HashMap<String, serde_json::Value> =
        serde_json::from_str(src).map_err(|e| format!("ошибка разбора JSON: {e}"))?;
    let mut map = HashMap::with_capacity(raw.len());
    for (k, v) in raw {
        let text = match v {
            serde_json::Value::String(s) => s,
            serde_json::Value::Array(a) => {
                let mut parts = Vec::with_capacity(a.len());
                for x in &a {
                    match x.as_str() {
                        Some(s) => parts.push(s),
                        None => return Err(format!("ключ {k}: элемент массива не строка")),
                    }
                }
                parts.join(" ")
            }
            other => {
                return Err(format!(
                    "ключ {k}: ожидалась строка или массив, получено {other}"
                ));
            }
        };
        map.insert(k, text);
    }
    Ok(map)
}

/// Карта вшитого бандла. Паника только на битом вшитом — он покрыт gate-тестом
/// полноты, поэтому в рантайме недостижима.
fn builtin_map(lang: Lang) -> HashMap<String, String> {
    json_to_map(lang.bundle_src()).unwrap_or_else(|e| panic!("вшитый бандл {lang:?}: {e}"))
}

/// Реестр только из вшитых бандлов — используется, когда [`init`] не вызывался
/// (тесты), и как база для [`init`].
fn build_builtin_registry() -> HashMap<Lang, &'static Locale> {
    Lang::ALL
        .iter()
        .map(|&lang| {
            let loc: &'static Locale = Box::leak(Box::new(Locale {
                lang,
                map: builtin_map(lang),
            }));
            (lang, loc)
        })
        .collect()
}

static BUILTIN: LazyLock<HashMap<Lang, &'static Locale>> = LazyLock::new(build_builtin_registry);
/// Полный реестр (вшитые + внешние), заполняется [`init`] один раз при старте.
static REGISTRY: OnceLock<HashMap<Lang, &'static Locale>> = OnceLock::new();

/// Мержит внешние `data/locales/*.json` в owned-карты бандлов: `<code>.json` поверх
/// вшитого того же кода (override по ключам) или новым языком. Нечитаемый/битый файл,
/// недопустимое имя → предупреждение в лог + пропуск (вшитое цело). Нет каталога —
/// no-op (только вшитые).
fn overlay_external(maps: &mut HashMap<Lang, HashMap<String, String>>, dir: &Path) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        // Код языка — непустая строка из строчных латинских букв (устойчиво к
        // случайным именам вроде `EN.json`/`readme.json`).
        if stem.is_empty() || !stem.bytes().all(|b| b.is_ascii_lowercase()) {
            tracing::warn!(
                file = %path.display(),
                "внешняя локаль: недопустимое имя (код языка — строчные латинские буквы), пропуск"
            );
            continue;
        }
        let src = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(file = %path.display(), error = %e, "внешняя локаль: чтение не удалось, пропуск");
                continue;
            }
        };
        let ext_map = match json_to_map(&src) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(file = %path.display(), error = %e, "внешняя локаль: разбор не удался, пропуск");
                continue;
            }
        };
        let lang = Lang::from_code(stem);
        let count = ext_map.len();
        let is_new = !maps.contains_key(&lang);
        let target = maps.entry(lang).or_default();
        for (k, v) in ext_map {
            target.insert(k, v); // override по ключам
        }
        if is_new {
            tracing::info!(
                lang = stem,
                keys = count,
                "внешняя локаль: добавлен новый язык (недостающие ключи — из референса ru)"
            );
        } else {
            tracing::info!(
                lang = stem,
                keys = count,
                "внешняя локаль: переопределены ключи вшитого бандла"
            );
        }
    }
}

/// Загружает внешние локали из каталога и фиксирует полный реестр. Вызывается один
/// раз при старте (`main.rs`) **до** первого обращения к [`locale`]. Повторный вызов
/// игнорируется. Нет каталога/файлов — реестр = вшитые бандлы.
pub fn init(dir: &Path) {
    let mut maps: HashMap<Lang, HashMap<String, String>> =
        Lang::ALL.iter().map(|&l| (l, builtin_map(l))).collect();
    overlay_external(&mut maps, dir);
    let registry: HashMap<Lang, &'static Locale> = maps
        .into_iter()
        .map(|(lang, map)| {
            let loc: &'static Locale = Box::leak(Box::new(Locale { lang, map }));
            (lang, loc)
        })
        .collect();
    let _ = REGISTRY.set(registry);
}

/// Активный реестр: полный (после [`init`]) либо только вшитый (тесты/до init).
fn registry() -> &'static HashMap<Lang, &'static Locale> {
    match REGISTRY.get() {
        Some(r) => r,
        None => &BUILTIN,
    }
}

/// Возвращает бандл языка (`&'static` — удобно класть в снимки хода/задач). Неизвестный
/// язык (внешний код без файла) → референсная локаль (`ru`) — мягкая деградация.
pub fn locale(lang: Lang) -> &'static Locale {
    let reg = registry();
    reg.get(&lang)
        .or_else(|| reg.get(&REFERENCE))
        .copied()
        .expect("референсная локаль (ru) всегда присутствует в реестре")
}

/// Бандл именно этого языка **без** фолбэка к референсу: `None`, если язык не
/// зарегистрирован (внешний код без файла). Нужен для `Lang::label` — фолбэк к ru
/// показал бы русское имя у чужого языка.
fn locale_exact(lang: Lang) -> Option<&'static Locale> {
    registry().get(&lang).copied()
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

    // ------- Внешние локали (Ярус 3): чистые тесты над локальной картой -------
    // Глобальный реестр (`init`/`REGISTRY`) в тестах НЕ трогаем — иначе один тест
    // подменил бы бандлы всему тест-бинарнику. Проверяем `overlay_external`/
    // `json_to_map`/`from_code`/`sort_langs` над передаваемыми данными.

    /// Строит owned-карты вшитых бандлов (как делает `init` перед оверлеем).
    fn builtin_maps() -> HashMap<Lang, HashMap<String, String>> {
        Lang::ALL.iter().map(|&l| (l, builtin_map(l))).collect()
    }

    #[test]
    fn json_to_map_parses_string_and_array() {
        let m = json_to_map(r#"{"a":"one","b":["two","three"]}"#).unwrap();
        assert_eq!(m["a"], "one");
        assert_eq!(m["b"], "two three"); // массив склеен пробелом
    }

    #[test]
    fn json_to_map_rejects_bad_value() {
        assert!(json_to_map(r#"{"a":42}"#).is_err());
        assert!(json_to_map("{ битый").is_err());
    }

    #[test]
    fn overlay_external_overrides_builtin_per_key() {
        // Частичный en.json переопределяет один существующий ключ; остальное — вшитое.
        let dir = tempfile::tempdir().unwrap();
        let key = "ui.feed.role.user"; // существует в обоих бандлах
        std::fs::write(
            dir.path().join("en.json"),
            format!("{{\"{key}\": \"OVERRIDDEN\"}}"),
        )
        .unwrap();
        let mut maps = builtin_maps();
        let before_other = maps[&Lang::En].len();
        overlay_external(&mut maps, dir.path());
        assert_eq!(maps[&Lang::En][key], "OVERRIDDEN");
        // Остальные ключи en остались (кол-во не уменьшилось — override, не замена).
        assert_eq!(maps[&Lang::En].len(), before_other);
        // ru не затронут.
        assert_ne!(maps[&Lang::Ru][key], "OVERRIDDEN");
    }

    #[test]
    fn overlay_external_adds_new_language() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("de.json"),
            r#"{"ui.lang.name": "Deutsch", "ui.feed.role.user": "DU"}"#,
        )
        .unwrap();
        let mut maps = builtin_maps();
        overlay_external(&mut maps, dir.path());
        let de = Lang::Ext("de");
        assert!(maps.contains_key(&de));
        assert_eq!(maps[&de]["ui.feed.role.user"], "DU");
        // Не включённый в файл ключ отсутствует в карте нового языка (фолбэк к ru —
        // на уровне `Locale::t`, покрыт `fallback_to_reference_then_key`).
        assert!(!maps[&de].contains_key("ui.feed.role.assistant"));
    }

    #[test]
    fn overlay_external_skips_malformed_and_bad_names() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("en.json"), "{ битый json").unwrap(); // битый
        std::fs::write(dir.path().join("DE.json"), r#"{"x":"y"}"#).unwrap(); // имя не lowercase
        std::fs::write(dir.path().join("readme.txt"), "not json").unwrap(); // не .json
        let mut maps = builtin_maps();
        let en_before = maps[&Lang::En].clone();
        overlay_external(&mut maps, dir.path());
        // Битый en.json пропущен — вшитое цело; неверные имена не добавили языков.
        assert_eq!(maps[&Lang::En], en_before);
        assert_eq!(maps.len(), Lang::ALL.len());
    }

    #[test]
    fn overlay_external_missing_dir_is_noop() {
        let mut maps = builtin_maps();
        let before = maps.clone();
        overlay_external(&mut maps, Path::new("no/such/locales/dir"));
        assert_eq!(maps.len(), before.len());
    }

    #[test]
    fn sort_langs_orders_ru_en_then_ext_by_code() {
        let mut v = vec![Lang::Ext("uk"), Lang::En, Lang::Ext("de"), Lang::Ru];
        sort_langs(&mut v);
        assert_eq!(
            v,
            vec![Lang::Ru, Lang::En, Lang::Ext("de"), Lang::Ext("uk")]
        );
    }

    #[test]
    fn code_and_from_code_round_trip() {
        assert_eq!(Lang::from_code("ru"), Lang::Ru);
        assert_eq!(Lang::from_code("en"), Lang::En);
        assert_eq!(Lang::from_code("de"), Lang::Ext("de"));
        for l in [Lang::Ru, Lang::En, Lang::Ext("de")] {
            assert_eq!(Lang::from_code(l.code()), l);
        }
    }

    #[test]
    fn lang_serde_round_trips_codes() {
        for (lang, json) in [(Lang::Ru, "\"ru\""), (Lang::En, "\"en\"")] {
            assert_eq!(serde_json::to_string(&lang).unwrap(), json);
            assert_eq!(serde_json::from_str::<Lang>(json).unwrap(), lang);
        }
        let de: Lang = serde_json::from_str("\"de\"").unwrap();
        assert_eq!(de, Lang::Ext("de"));
        assert_eq!(serde_json::to_string(&de).unwrap(), "\"de\"");
    }

    #[test]
    fn label_ext_falls_back_to_code_without_bundle() {
        // Внешний язык без загруженного бандла (реестр в тестах — только вшитые) →
        // фолбэк на код: `locale_exact` не находит `de`, референс не подставляется.
        assert_eq!(Lang::Ext("de").label(), "de");
    }

    #[test]
    fn loaded_external_language_registers_and_self_names() {
        // Полный путь `init` без глобального реестра: overlay → leak в локальную карту
        // → `all_from`/`get`/`t`. Покрывает связку, которую чистые тесты не трогают.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("de.json"),
            r#"{"ui.lang.name":"Deutsch","ui.feed.role.user":"DU"}"#,
        )
        .unwrap();
        let mut maps = builtin_maps();
        overlay_external(&mut maps, dir.path());
        let reg: HashMap<Lang, &'static Locale> = maps
            .into_iter()
            .map(|(lang, map)| {
                let loc: &'static Locale = Box::leak(Box::new(Locale { lang, map }));
                (lang, loc)
            })
            .collect();
        let de = Lang::Ext("de");
        // Новый язык зарегистрирован в порядке ru, en, de.
        assert_eq!(all_from(&reg), vec![Lang::Ru, Lang::En, de]);
        // Самоименование из своего бандла (источник данных для `Lang::label` у Ext).
        assert_eq!(reg[&de].get("ui.lang.name"), Some("Deutsch"));
        // Свой ключ из файла.
        assert_eq!(reg[&de].t("ui.feed.role.user"), "DU");
        // Ключ вне файла → фолбэк к референсу ru (не пусто, не сам ключ).
        assert_eq!(
            reg[&de].t("ui.feed.role.assistant"),
            reg[&Lang::Ru].t("ui.feed.role.assistant")
        );
    }
}
