//! Мультиязычность **служебного каркаса агента** (i18n, ось A): промпты фоновых
//! задач, каркас «модели себя», результаты инструментов — тексты, которые читает
//! *модель*. Язык — свойство профиля (`Profile.language`, см. [docs/history/i18n.md]). Язык
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
//! **Внешние локали (Ярус 3, [docs/history/i18n-external-locales.md]).** При старте
//! [`init`] сканирует `data/locales/*.json`: файл `<code>.json` (код BCP-47-подобный
//! — `de`, `pt-br`, `zh-tw`) мержится **поверх** вшитого бандла того же кода
//! (частичный override — переопределяются только присутствующие ключи), а файл с
//! новым кодом добавляет **новый язык** ([`Lang::Ext`]) без пересборки. Недостающие
//! ключи резолвятся по цепочке: свой бандл → заявленный мета-ключом `_fallback`
//! (напр. `"_fallback":"en"` для языка, переведённого с английского) → референс `ru`
//! → сам ключ. Битый/нечитаемый файл или недопустимое имя — предупреждение в лог +
//! пропуск (мягкая деградация: вшитое цело); содержимое дополнительно валидируется
//! против референса (неизвестные ключи, расхождение плейсхолдеров → warn, не отбраковка).
//! Исчерпание цепочки (ключа нет нигде → в вывод уходит слаг) логируется один раз на
//! ключ — единственный сигнал этого дефекта для внешних локалей без гейт-тестов.
//! Экспорт бандла-шаблона — [`export_bundle`] (CLI `mindfork locales export`).
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

/// Уже пожаловавшиеся `(язык, ключ)` — чтобы исчерпание фолбэка логировалось один раз
/// на ключ, а не на каждый рендер кадра (`t` на горячем пути).
static WARNED_MISSING: LazyLock<Mutex<HashSet<(Lang, String)>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Логирует «ключ исчерпал цепочку фолбэка → в вывод уходит слаг» один раз на ключ.
/// Вызывается только на реально пропущенном ключе (редкий путь), лок не мешает
/// happy-path `t`.
fn warn_missing_key_once(lang: Lang, key: &str) {
    let mut warned = WARNED_MISSING
        .lock()
        .expect("набор предупреждений отравлен");
    if warned.insert((lang, key.to_string())) {
        tracing::warn!(
            lang = lang.code(),
            key,
            "i18n: ключ отсутствует во всех бандлах цепочки — в вывод уйдёт сам ключ (слаг)"
        );
    }
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
            .filter(|s| !s.is_empty()) // пустой override не должен дать пустую метку
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

/// Язык по строке локали ОС (напр. `"ru-RU"`, `"en_US.UTF-8"`): берётся первичный
/// субтег (до `-`/`_`/`.`), `ru` → [`Lang::Ru`], всё прочее (в т.ч. `None`) → [`Lang::En`]
/// как международный дефолт. Вынесено чистой функцией ради тестируемости (сам вызов
/// `get_locale` от ОС не воспроизводим в тесте). Внешние языки (Ярус 3) здесь пока не
/// распознаются — реестр на момент вызова ([`Paths::resolve`] в peek-фазе) ещё не
/// инициализирован; расширение — задел (docs/roadmap.md, «Определение языка по локали ОС»).
pub fn lang_for_locale(locale: Option<&str>) -> Lang {
    let primary = locale.and_then(|l| l.split(['-', '_', '.']).next());
    match primary.map(str::to_ascii_lowercase).as_deref() {
        Some("ru") => Lang::Ru,
        _ => Lang::En,
    }
}

/// Язык интерфейса по локали ОС — для свежей установки без `defaults.json`/`settings.json`
/// (голый портативный zip, deb/rpm-пакет, где выбор языка при установке невозможен, §4.3
/// docs/history/installers.md). Делегирует [`lang_for_locale`]; при недоступной локали —
/// `En`.
pub fn detect_os_language() -> Lang {
    lang_for_locale(sys_locale::get_locale().as_deref())
}

/// Загруженный бандл одного языка: плоская таблица «ключ → текст».
pub struct Locale {
    lang: Lang,
    map: HashMap<String, String>,
    /// Заявленный язык-фолбэк из мета-ключа `_fallback` внешнего файла (напр. немецкая
    /// локаль, переведённая с английского, ставит `"_fallback": "en"` — недостающие
    /// ключи берутся из en, а не из русского референса). `None` у вшитых. Цепочка
    /// резолва: свой бандл → этот фолбэк → референс (`ru`) → сам ключ.
    fallback: Option<Lang>,
}

impl Locale {
    /// Строит локаль из сырой карты, извлекая мета-ключ `_fallback` (он не
    /// переводится и не участвует в гейтах/выдаче). Единственная точка сборки
    /// `Locale` — так `_fallback` обрабатывается одинаково для вшитых и внешних.
    fn from_map(lang: Lang, mut map: HashMap<String, String>) -> Locale {
        let fallback = map.remove("_fallback").map(|c| Lang::from_code(c.trim()));
        Locale {
            lang,
            map,
            fallback,
        }
    }

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

    /// Значение ключа. Фолбэк: этот язык → заявленный `_fallback` → референсный
    /// (`ru`) → сам ключ. Никогда не паникует — пропущенный ключ деградирует, а не
    /// роняет промпт. Штатный фолбэк неполного бандла (ключ есть в референсе) молчит;
    /// **исчерпание** цепочки (ключа нет нигде → в вывод уйдёт слаг) логируется один
    /// раз на ключ — единственный сигнал этого дефекта для внешних локалей без гейтов.
    pub fn t<'a>(&'a self, key: &'a str) -> &'a str {
        if let Some(v) = self.map.get(key) {
            return v;
        }
        // Заявленный язык-фолбэк (мета `_fallback`), затем референс — прямым доступом к
        // их картам (без рекурсии через `t`): один переход к каждому, не цепочка вызовов.
        if let Some(fb) = self.fallback
            && fb != self.lang
            && let Some(v) = locale(fb).map.get(key)
        {
            return v;
        }
        if self.lang != REFERENCE
            && self.fallback != Some(REFERENCE)
            && let Some(v) = locale(REFERENCE).map.get(key)
        {
            return v;
        }
        warn_missing_key_once(self.lang, key);
        key
    }

    /// Значение существующего ключа как `&'static` (для динамических ключей вида
    /// `ui.tool.label.{id}`). Требует `&'static self` — оба конца статичны. Без
    /// фолбэка: `None`, если ключа нет (вызывающий подставит русский `&'static`
    /// fallback).
    pub fn get(&'static self, key: &str) -> Option<&'static str> {
        self.map.get(key).map(|s| s.as_str())
    }

    /// Значение с подстановкой именованных плейсхолдеров `{name}`. **Однопроходная**
    /// замена: значение аргумента подставляется дословно и НЕ пере-сканируется,
    /// поэтому `{плейсхолдер}` внутри значения не раскрывается (устраняет каскадную
    /// ре-подстановку — критично, когда значение содержит `{…}`: контент страницы у
    /// `fetch_url`, текст `policy_core` в `{core}`). Без plural-правил — формулировки
    /// нейтральны к числу («×{n}», «заметок: {n}»).
    pub fn tf(&self, key: &str, args: &[(&str, &str)]) -> String {
        let (out, unused) = substitute(self.t(key), args);
        // Дрейф код↔бандл: переданный аргумент, которого нет в шаблоне — почти всегда
        // переименованный/забытый плейсхолдер (в бандле `{count}`, код шлёт `{n}`).
        // В release проверка скомпилирована прочь.
        debug_assert!(
            unused.is_empty(),
            "tf(\"{key}\"): аргументы не встретились в шаблоне: {unused:?} — плейсхолдер переименован?"
        );
        out
    }
}

/// Однопроходная подстановка `{name}` из `args`. Возвращает результат и имена
/// аргументов, не встретившихся в шаблоне (для debug-проверки дрейфа в [`Locale::tf`]).
/// Неизвестный/битый `{…}` копируется дословно (мягкая деградация).
fn substitute<'a>(template: &str, args: &'a [(&'a str, &'a str)]) -> (String, Vec<&'a str>) {
    let mut used = vec![false; args.len()];
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        // Имя плейсхолдера — до ближайшей `}`, без вложенной `{` (иначе это не он).
        match after.find('}') {
            Some(close) if !after[..close].contains('{') && !after[..close].is_empty() => {
                let name = &after[..close];
                match args.iter().position(|(k, _)| *k == name) {
                    Some(idx) => {
                        out.push_str(args[idx].1);
                        used[idx] = true;
                    }
                    // Плейсхолдер без аргумента — оставляем дословно `{name}`.
                    None => {
                        out.push('{');
                        out.push_str(name);
                        out.push('}');
                    }
                }
                rest = &after[close + 1..];
            }
            // Одинокая `{` без пары — копируем и идём дальше.
            _ => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    let unused = args
        .iter()
        .zip(&used)
        .filter(|(_, u)| !**u)
        .map(|((k, _), _)| *k)
        .collect();
    (out, unused)
}

/// Мета-ключ внешнего файла, задающий язык-фолбэк (не переводимый ключ; извлекается
/// в [`Locale::from_map`], исключается из валидации/гейтов/экспорта).
const FALLBACK_META_KEY: &str = "_fallback";

/// Множество имён плейсхолдеров `{name}` в строке — для gate-теста и рантайм-валидации
/// внешних файлов (набор плейсхолдеров переопределённого ключа обязан совпадать с
/// референсом, иначе `tf` оставит дыру/проигнорирует аргумент).
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

/// Валиден ли код языка для имени файла внешней локали. BCP-47-подобный: строчные
/// латинские буквы, цифры и дефис-разделители подтегов; начинается с буквы, не
/// оканчивается дефисом, без двойных дефисов (`de`, `pt-br`, `zh-tw`, `sr-latn`).
fn is_valid_lang_code(code: &str) -> bool {
    let bytes = code.as_bytes();
    if bytes.is_empty() || !bytes[0].is_ascii_lowercase() || *bytes.last().unwrap() == b'-' {
        return false;
    }
    bytes.windows(2).all(|w| w != b"--")
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
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
            let loc: &'static Locale =
                Box::leak(Box::new(Locale::from_map(lang, builtin_map(lang))));
            (lang, loc)
        })
        .collect()
}

static BUILTIN: LazyLock<HashMap<Lang, &'static Locale>> = LazyLock::new(build_builtin_registry);
/// Полный реестр (вшитые + внешние), заполняется [`init`] один раз при старте.
static REGISTRY: OnceLock<HashMap<Lang, &'static Locale>> = OnceLock::new();

/// Мержит внешние `data/locales/*.json` в owned-карты бандлов: `<code>.json` поверх
/// вшитого того же кода (override по ключам) или новым языком. Нечитаемый/битый файл,
/// недопустимое имя → предупреждение + пропуск (вшитое цело). Нет каталога — no-op
/// (только вшитые). Возвращает предупреждения (проблемные файлы/ключи): [`init`]
/// вызывается **до** установки лог-подписчика, поэтому предупреждения не пишутся в
/// `tracing` здесь, а возвращаются вызывающему и логируются им после `logging::init`
/// (ветки раннего выхода — `--help` — их молча отбрасывают). Информационные события
/// (добавлен язык/переопределены ключи) остаются `tracing::info!` (не критичны, если
/// потеряны при раннем старте).
fn overlay_external(maps: &mut HashMap<Lang, HashMap<String, String>>, dir: &Path) -> Vec<String> {
    let mut warnings = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return warnings,
    };
    // Снимок референса (ru) для содержательной валидации внешних файлов: множество
    // ключей + плейсхолдеры каждого. Берём до цикла (в цикле `maps` мутируется).
    let ref_placeholders: HashMap<String, std::collections::BTreeSet<String>> = maps
        .get(&REFERENCE)
        .map(|m| {
            m.iter()
                .map(|(k, v)| (k.clone(), placeholders(v)))
                .collect()
        })
        .unwrap_or_default();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        // Код языка — BCP-47-подобный (строчные буквы/цифры/дефис, начинается с буквы):
        // устойчиво к случайным именам (`EN.json`/`readme.json`), допускает `pt-br`.
        if !is_valid_lang_code(stem) {
            warnings.push(format!(
                "внешняя локаль {}: недопустимое имя (код языка — строчные латинские буквы/цифры/дефис, с буквы), пропуск",
                path.display()
            ));
            continue;
        }
        let src = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                warnings.push(format!(
                    "внешняя локаль {}: чтение не удалось ({e}), пропуск",
                    path.display()
                ));
                continue;
            }
        };
        let ext_map = match json_to_map(&src) {
            Ok(m) => m,
            Err(e) => {
                warnings.push(format!(
                    "внешняя локаль {}: разбор не удался ({e}), пропуск",
                    path.display()
                ));
                continue;
            }
        };
        // Содержательная валидация против референса (зеркало gate-тестов parity —
        // единственная категория бандлов без тестового покрытия): предупреждаем, но
        // не отбрасываем (пользователь мог править экспериментально).
        warnings.extend(validate_external_map(&path, &ext_map, &ref_placeholders));
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
    warnings
}

/// Проверяет внешний бандл против референса и возвращает предупреждения (не
/// отбрасывает): (1) ключ, отсутствующий в референсе, — вероятная опечатка: он ничего
/// не переопределяет и мёртв (для нового языка тоже: недостающих ключей быть не должно,
/// а лишних — тем более); (2) переопределённый ключ с другим набором `{плейсхолдеров}`,
/// чем в референсе, — сломает `tf` (дыра/проигнорированный аргумент). Мета-ключ
/// `_fallback` из проверки исключён. Возвращает строки для логирования вызывающим
/// (см. [`overlay_external`] — предупреждения не пишутся здесь, т.к. `init` идёт до
/// установки лог-подписчика).
fn validate_external_map(
    path: &Path,
    ext_map: &HashMap<String, String>,
    ref_placeholders: &HashMap<String, std::collections::BTreeSet<String>>,
) -> Vec<String> {
    let mut warnings = Vec::new();
    for (k, v) in ext_map {
        if k == FALLBACK_META_KEY {
            continue;
        }
        match ref_placeholders.get(k) {
            None => warnings.push(format!(
                "внешняя локаль {} ключ {k}: нет в референсе (опечатка? ключ ничего не переопределяет)",
                path.display()
            )),
            Some(want) => {
                let got = placeholders(v);
                if &got != want {
                    warnings.push(format!(
                        "внешняя локаль {} ключ {k}: набор плейсхолдеров расходится с референсом (want {want:?}, got {got:?}) — сломает подстановку tf",
                        path.display()
                    ));
                }
            }
        }
    }
    warnings
}

/// Загружает внешние локали из каталога и фиксирует полный реестр. Вызывается один
/// раз при старте (`main.rs`) **до** первого обращения к [`locale`]. Повторный вызов
/// игнорируется. Нет каталога/файлов — реестр = вшитые бандлы.
///
/// Возвращает предупреждения о проблемных внешних файлах/ключах: `init` идёт **до**
/// установки лог-подписчика (`logging::init`), поэтому `tracing::warn!` здесь бы
/// потерялся — вызывающий логирует их сам после инициализации логов (ветки раннего
/// выхода вроде `--help` отбрасывают). Повторный вызов возвращает пустой список.
#[must_use]
pub fn init(dir: &Path) -> Vec<String> {
    let mut maps: HashMap<Lang, HashMap<String, String>> =
        Lang::ALL.iter().map(|&l| (l, builtin_map(l))).collect();
    let warnings = overlay_external(&mut maps, dir);
    let registry: HashMap<Lang, &'static Locale> = maps
        .into_iter()
        .map(|(lang, map)| {
            let loc: &'static Locale = Box::leak(Box::new(Locale::from_map(lang, map)));
            (lang, loc)
        })
        .collect();
    if REGISTRY.set(registry).is_err() {
        return Vec::new();
    }
    warnings
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

/// Содержимое бандла для экспорта в файл-шаблон (CLI `mindfork locales export`).
/// Вшитые `ru`/`en` — исходный JSON **дословно** (сохраняет массивы/форматирование,
/// удобно править). Иначе — полный набор ключей референса со значениями, разрешёнными
/// для языка (JSON, ключи отсортированы): для зарегистрированного внешнего языка это
/// его значения + ru-дыры, для нового кода — целиком ru (шаблон для перевода).
pub fn export_bundle(lang: Lang) -> String {
    if let Lang::Ru | Lang::En = lang {
        return lang.bundle_src().to_string();
    }
    let reference = locale(REFERENCE);
    let target = locale(lang);
    let mut keys: Vec<&String> = reference.map.keys().collect();
    keys.sort();
    let obj: serde_json::Map<String, serde_json::Value> = keys
        .into_iter()
        .map(|k| {
            (
                k.clone(),
                serde_json::Value::String(target.t(k).to_string()),
            )
        })
        .collect();
    serde_json::to_string_pretty(&serde_json::Value::Object(obj)).unwrap_or_default()
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
    fn lang_for_locale_maps_primary_subtag() {
        // Русская локаль в разных формах → Ru; всё прочее и None → En (межд. дефолт).
        assert_eq!(lang_for_locale(Some("ru")), Lang::Ru);
        assert_eq!(lang_for_locale(Some("ru-RU")), Lang::Ru);
        assert_eq!(lang_for_locale(Some("ru_RU.UTF-8")), Lang::Ru);
        assert_eq!(lang_for_locale(Some("RU")), Lang::Ru);
        assert_eq!(lang_for_locale(Some("en-US")), Lang::En);
        assert_eq!(lang_for_locale(Some("de-DE")), Lang::En);
        assert_eq!(lang_for_locale(Some("")), Lang::En);
        assert_eq!(lang_for_locale(None), Lang::En);
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
        // подстановка `tf` оставит дыру или проигнорирует аргумент. Используем
        // модульный `placeholders` (та же логика питает рантайм-валидацию внешних).
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
        // Прямой прокси go-критерия (docs/history/i18n.md, Ярус 1): en-каркас не содержит
        // кириллицы. Ловит случайно оставленный русский текст в переводе. tool-имена
        // и ключи — ASCII, так что чистый en-бандл сплошь латиница/пунктуация.
        for (key, val) in &locale(Lang::En).map {
            let cyr = val.chars().find(|&c| {
                ('а'..='я').contains(&c) || ('А'..='Я').contains(&c) || c == 'ё' || c == 'Ё'
            });
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

    /// Собирает все строковые литералы вида `"<seg>.<seg>…"` (2+ сегмента из
    /// `[a-z0-9_]`, разделённых точками, целиком между кавычками) из всех `.rs` под
    /// `src/`. Общий сканер для прямого и обратного гейтов ключей.
    fn dotted_literals_in_src() -> std::collections::BTreeSet<String> {
        use std::path::Path;
        fn scan(s: &str, out: &mut std::collections::BTreeSet<String>) {
            let bytes = s.as_bytes();
            let mut i = 0;
            while i < bytes.len() {
                let Some(p) = s[i..].find('"') else { break };
                let start = i + p + 1; // за открывающей кавычкой; всегда > i (прогресс)
                let mut j = start;
                let mut dots = 0usize;
                while j < bytes.len() {
                    let c = bytes[j] as char;
                    if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' {
                        j += 1;
                    } else if c == '.' {
                        dots += 1;
                        j += 1;
                    } else {
                        break;
                    }
                }
                // Литерал целиком (следом кавычка), ≥2 сегмента, не оканчивается точкой.
                if j < bytes.len()
                    && bytes[j] as char == '"'
                    && dots >= 1
                    && bytes[j - 1] as char != '.'
                {
                    out.insert(s[start..j].to_string());
                }
                i = j; // j ≥ start > прежний i — цикл всегда продвигается
            }
        }
        fn visit(dir: &Path, out: &mut std::collections::BTreeSet<String>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    visit(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    scan(&std::fs::read_to_string(&path).unwrap_or_default(), out);
                }
            }
        }
        let mut out = std::collections::BTreeSet::new();
        visit(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut out);
        out
    }

    /// Верхнеуровневые префиксы (`ui`, `tool`, …), реально присутствующие в бандле —
    /// по ним отличаем ключи локали от прочих дотированных литералов (пути и т. п.).
    fn bundle_prefixes() -> std::collections::BTreeSet<String> {
        locale(REFERENCE)
            .map
            .keys()
            .filter_map(|k| k.split('.').next().map(str::to_string))
            .collect()
    }

    #[test]
    fn all_bundle_key_references_in_code_exist() {
        // Гейт против класса бага «код зовёт loc.t("tool.…"/"ui.…"), а ключа нет в
        // бандле» (тогда `t` молча возвращает сам ключ — слаг в промпт модели / UI).
        // Раньше покрывались только `ui.*`; теперь — ВСЯ ось A (`tool.`/`selfmodel.`/
        // `notes.`/`prompt.`/…). Динамические ключи (`format!`) — не литералы, покрыты
        // отдельно (meta.rs). Имена файлов с бандл-префиксом — в whitelist.
        const NON_KEY: &[&str] = &["defaults.json", "python.webc"];
        let prefixes = bundle_prefixes();
        let ru = locale(Lang::Ru);
        let missing: Vec<String> = dotted_literals_in_src()
            .into_iter()
            .filter(|k| {
                prefixes.contains(k.split('.').next().unwrap()) && !NON_KEY.contains(&k.as_str())
            })
            .filter(|k| !ru.has_key(k))
            .collect();
        assert!(
            missing.is_empty(),
            "ключи есть в коде, но отсутствуют в бандле: {missing:?}"
        );
    }

    #[test]
    fn bundle_keys_are_not_dead() {
        // Обратный гейт: каждый ключ бандла реально упомянут в коде (литералом), иначе
        // это мёртвый ключ (опечатка/остаток рефактора). Динамические семейства
        // (`ui.tool.label.<id>` — собираются `format!`) исключены whitelist-префиксом.
        const DYNAMIC_PREFIX: &[&str] = &["ui.tool.label."];
        let literals = dotted_literals_in_src();
        let dead: Vec<&String> = locale(REFERENCE)
            .map
            .keys()
            .filter(|k| !literals.contains(*k))
            .filter(|k| !DYNAMIC_PREFIX.iter().any(|p| k.starts_with(p)))
            .collect();
        assert!(
            dead.is_empty(),
            "ключи бандла нигде не используются в коде (мёртвые?): {dead:?}"
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

    // ------- Однопроходная подстановка `tf` / `substitute` -------

    #[test]
    fn substitute_is_single_pass_no_cascade() {
        // Значение аргумента `a` содержит плейсхолдер `{b}` — он НЕ должен раскрыться
        // (иначе контент страницы/`policy_core` с `{…}` вызвал бы каскад).
        let (out, unused) = substitute("{a}{b}", &[("a", "{b}"), ("b", "X")]);
        assert_eq!(out, "{b}X");
        assert!(unused.is_empty());
    }

    #[test]
    fn substitute_leaves_unknown_placeholder_verbatim() {
        let (out, unused) = substitute("{x} {a} {", &[("a", "1")]);
        assert_eq!(out, "{x} 1 {"); // неизвестный `{x}` и одинокая `{` — дословно
        assert!(unused.is_empty());
    }

    #[test]
    fn substitute_reports_unused_args() {
        // Переданный аргумент, которого нет в шаблоне (дрейф код↔бандл) — в `unused`.
        let (_out, unused) = substitute("{a}", &[("a", "1"), ("z", "2")]);
        assert_eq!(unused, vec!["z"]);
    }

    #[test]
    fn tf_does_not_cascade_on_real_key() {
        // `selfmodel.maintenance_wrapper` = "(… {core})"; подставляем значение с `{n}` —
        // оно не должно раскрыться (нет каскада), `core` использован (нет unused).
        let ru = locale(Lang::Ru);
        let s = ru.tf("selfmodel.maintenance_wrapper", &[("core", "A {n} B")]);
        assert!(s.contains("A {n} B"), "{s}");
    }

    // ------- Код языка / внешние локали -------

    #[test]
    fn is_valid_lang_code_accepts_bcp47_and_rejects_junk() {
        for ok in ["de", "en", "pt-br", "zh-tw", "sr-latn", "x9"] {
            assert!(is_valid_lang_code(ok), "должен принять: {ok}");
        }
        for bad in ["", "EN", "e n", "-de", "de-", "d--e", "1de", "de.json"] {
            assert!(!is_valid_lang_code(bad), "должен отвергнуть: {bad}");
        }
    }

    #[test]
    fn from_map_extracts_fallback_meta() {
        let mut m = HashMap::new();
        m.insert("_fallback".to_string(), "en".to_string());
        m.insert("k".to_string(), "v".to_string());
        let loc = Locale::from_map(Lang::Ext("de"), m);
        assert_eq!(loc.fallback, Some(Lang::En));
        assert!(!loc.map.contains_key("_fallback")); // мета-ключ не в переводимых
        assert_eq!(loc.map.get("k").map(String::as_str), Some("v"));
    }

    #[test]
    fn external_fallback_meta_drives_t_before_reference() {
        // Локаль `de` с `_fallback: en` и пустыми переводами: ключ, отсутствующий у
        // неё, должен браться из EN (глобальный BUILTIN), а НЕ из русского референса.
        let mut m = HashMap::new();
        m.insert("_fallback".to_string(), "en".to_string());
        let de = Locale::from_map(Lang::Ext("de"), m);
        let key = "ui.feed.role.user"; // ru "ВЫ" ≠ en "YOU"
        assert_eq!(de.t(key), locale(Lang::En).t(key));
        assert_ne!(de.t(key), locale(Lang::Ru).t(key));
    }

    #[test]
    fn label_filters_empty_override() {
        // Внешний override `"ui.lang.name": ""` не должен дать пустую метку —
        // фильтр отбрасывает пустое значение (механизм, на который опирается `label`).
        let mut m = HashMap::new();
        m.insert("ui.lang.name".to_string(), String::new());
        let loc: &'static Locale = Box::leak(Box::new(Locale::from_map(Lang::Ext("de"), m)));
        assert_eq!(loc.get("ui.lang.name").filter(|s| !s.is_empty()), None);
    }

    #[test]
    fn overlay_validation_is_non_fatal() {
        // Внешний файл с неизвестным ключом (нет в референсе) и расходящимися
        // плейсхолдерами: `validate_external_map` предупреждает (лог), но overlay
        // всё равно мержит содержимое (валидация — совет, не отбраковка).
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("de.json"),
            r#"{"totally.unknown.key":"x","selfmodel.age.days":"vor {tagen} Tagen"}"#,
        )
        .unwrap();
        let mut maps = builtin_maps();
        overlay_external(&mut maps, dir.path());
        let de = &maps[&Lang::Ext("de")];
        assert_eq!(de.get("totally.unknown.key").map(String::as_str), Some("x"));
        assert!(de.contains_key("selfmodel.age.days")); // смёржено несмотря на warn
    }

    #[test]
    fn overlay_accepts_bcp47_named_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("pt-br.json"),
            r#"{"ui.lang.name":"Português (BR)"}"#,
        )
        .unwrap();
        let mut maps = builtin_maps();
        overlay_external(&mut maps, dir.path());
        assert!(maps.contains_key(&Lang::Ext("pt-br")));
    }

    // ------- Экспорт бандла (CLI locales export) -------

    #[test]
    fn export_builtin_is_raw_source() {
        // Вшитый язык экспортируется дословным исходником (сохраняет массивы).
        assert_eq!(export_bundle(Lang::Ru), Lang::Ru.bundle_src());
        assert_eq!(export_bundle(Lang::En), Lang::En.bundle_src());
    }

    #[test]
    fn export_unknown_code_yields_reference_template() {
        // Незарегистрированный код → шаблон: полный набор ключей референса со
        // значениями ru (валидный JSON-объект, все ключи на месте).
        let json = export_bundle(Lang::Ext("zz"));
        let obj: HashMap<String, String> = serde_json::from_str(&json).unwrap();
        let ru = locale(Lang::Ru);
        assert_eq!(obj.len(), ru.map.len());
        assert_eq!(obj.get("ui.lang.name"), ru.map.get("ui.lang.name"));
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
                let loc: &'static Locale = Box::leak(Box::new(Locale::from_map(lang, map)));
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
