//! Речевой экстрактор markdown → простой текст для синтеза речи (TTS).
//! Правила — таблица docs/research/tts.md §5 (источник истины).
//!
//! Это **второй потребитель** тех же событий `pulldown-cmark`, что и [`Writer`]
//! (ADR 0003): рендер ленты рисует, экстрактор — озвучивает. Общий у них только
//! парсер и чистые хелперы LaTeX ([`latex_to_unicode`], [`normalize_delimiters`]);
//! `Writer` не трогаем — свой walker проще и не тянет стили/ширину/палитру.
//!
//! Что делает экстрактор (см. §5):
//! - блоки кода (в т.ч. ` ```mermaid `), таблицы и блочные формулы `$$…$$`
//!   **пропускаются** с короткой голосовой пометкой на языке профиля (ось A i18n);
//! - inline-математика конвертируется в unicode, inline-код читается текстом;
//! - у ссылки читается только текст, голый URL/автолинк заменяется «ссылка: домен»;
//! - у изображения читается alt (иначе пометка);
//! - заголовки/списки/цитаты читаются текстом, на границах ставится точка/перенос
//!   строки — чтобы движок сделал паузу.
//!
//! Функция чистая (без движка/аудио) — покрыта golden-тестами.

use super::*;
use crate::shared::i18n::Locale;

/// Извлекает из markdown текст, пригодный для озвучивания.
///
/// `loc` — локаль **языка профиля** (ось A): голосовые пометки о пропущенных
/// блоках — это часть речевого контента, а не UI-хром.
///
/// Пустой/пробельный вход даёт пустую строку (вызывающему нечего синтезировать).
// Потребители (команда `/tts`, оркестрация озвучивания) появятся следующими
// этапами направления TTS; пока функцию зовут только тесты — как фасад `render`.
#[allow(dead_code)]
pub fn speakable_text(markdown: &str, loc: &'static Locale) -> String {
    let normalized = normalize_delimiters(markdown);
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    opts.insert(Options::ENABLE_MATH);
    opts.insert(Options::ENABLE_TABLES);
    let mut speaker = Speaker::new(loc);
    for event in Parser::new_ext(&normalized, opts) {
        speaker.handle(event);
    }
    speaker.finish()
}

/// Накопитель речевых «блоков»: каждый блок — законченная фраза, блоки
/// разделяются переводом строки (пауза у любого движка).
struct Speaker {
    loc: &'static Locale,
    blocks: Vec<String>,
    /// Текущий (незавершённый) блок.
    cur: String,
    /// Идёт пропуск блока кода (содержимое не озвучивается).
    skip_code: bool,
    /// Идёт пропуск таблицы (ячейки не озвучиваются).
    skip_table: bool,
    /// Сбор alt-текста изображения (внутри `![…](…)`).
    image_alt: Option<String>,
}

impl Speaker {
    fn new(loc: &'static Locale) -> Self {
        Self {
            loc,
            blocks: Vec::new(),
            cur: String::new(),
            skip_code: false,
            skip_table: false,
            image_alt: None,
        }
    }

    fn handle(&mut self, event: Event<'_>) {
        // В пропускаемом блоке ждём только его конца — содержимое не озвучивается.
        if self.skip_code {
            if matches!(event, Event::End(TagEnd::CodeBlock)) {
                self.skip_code = false;
            }
            return;
        }
        if self.skip_table {
            if matches!(event, Event::End(TagEnd::Table)) {
                self.skip_table = false;
            }
            return;
        }
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.text(&text),
            // Инлайн-код читается обычным текстом (короткие идентификаторы в речи
            // полезны), URL-подстановка к нему не применяется.
            Event::Code(code) => self.push(&code),
            Event::InlineMath(content) => {
                if looks_like_price_fragment(&content) {
                    // Ложное срабатывание math на диапазоне цен «$5-$10» — читаем как есть.
                    let literal = format!("${content}$");
                    self.push(&literal);
                } else {
                    let converted = latex_to_unicode(&content);
                    self.push(&converted);
                }
            }
            // Блочная формула не читается (озвучивать «\begin{aligned}» бессмысленно).
            Event::DisplayMath(_) => self.note("speak.skip.formula"),
            Event::SoftBreak => self.push(" "),
            Event::HardBreak => self.flush(),
            Event::InlineHtml(html) | Event::Html(html) if is_br(&html) => self.flush(),
            Event::Rule => self.flush(),
            // Прочий HTML, сноски, маркеры задач — молча пропускаем.
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::CodeBlock(kind) => {
                let info = match kind {
                    CodeBlockKind::Fenced(ref lang) => lang.as_ref(),
                    CodeBlockKind::Indented => "",
                };
                // ` ```mermaid ` — обычный код-блок с инфо-строкой; отдельного детекта
                // не нужно, только своя пометка (диаграмма, а не код).
                let lang = info.split([',', ' ', '\t']).next().unwrap_or("");
                let key = if lang.eq_ignore_ascii_case("mermaid") {
                    "speak.skip.mermaid"
                } else {
                    "speak.skip.code"
                };
                self.note(key);
                self.skip_code = true;
            }
            Tag::Table(_) => {
                self.note("speak.skip.table");
                self.skip_table = true;
            }
            // Начало изображения: alt придёт событиями `Text`, перехватываем их.
            Tag::Image { .. } => self.image_alt = Some(String::new()),
            // Границы блоков — пауза (точка/перенос строки).
            Tag::Paragraph
            | Tag::Heading { .. }
            | Tag::BlockQuote(_)
            | Tag::List(_)
            | Tag::Item => self.flush(),
            // Ссылка: URL не читаем, текст ссылки придёт событиями `Text`.
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Image => {
                let alt = self.image_alt.take().unwrap_or_default();
                if alt.trim().is_empty() {
                    let note = self.loc.t("speak.skip.image").to_string();
                    self.push(&note);
                } else {
                    self.push(&alt);
                }
            }
            TagEnd::Paragraph
            | TagEnd::Heading(_)
            | TagEnd::BlockQuote(_)
            | TagEnd::List(_)
            | TagEnd::Item => self.flush(),
            _ => {}
        }
    }

    fn text(&mut self, text: &str) {
        if let Some(alt) = &mut self.image_alt {
            alt.push_str(text);
            return;
        }
        // Голый URL и автолинк (`<https://…>`, текст которого — сам URL) читаются
        // как «ссылка: домен».
        let spoken = rewrite_urls(text, self.loc);
        self.push(&spoken);
    }

    fn push(&mut self, text: &str) {
        self.cur.push_str(text);
    }

    /// Закрывает текущий блок: схлопывает пробелы и добивает точкой (пауза),
    /// если фраза не заканчивается знаком препинания. Пустой блок отбрасывается.
    fn flush(&mut self) {
        let collapsed = collapse_ws(&self.cur);
        self.cur.clear();
        if collapsed.is_empty() {
            return;
        }
        let ends_sentence = collapsed.ends_with(['.', '!', '?', ':', ';', '…']);
        self.blocks.push(if ends_sentence {
            collapsed
        } else {
            format!("{collapsed}.")
        });
    }

    /// Голосовая пометка о пропущенном блоке — отдельным блоком (пауза с обеих сторон).
    fn note(&mut self, key: &str) {
        self.flush();
        let note = self.loc.t(key).to_string();
        self.push(&note);
        self.flush();
    }

    fn finish(mut self) -> String {
        self.flush();
        self.blocks.join("\n")
    }
}

/// Схлопывает любые пробельные последовательности в один пробел и подрезает края.
fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Заменяет голые http(s)-URL на «ссылка: домен», сохраняя окружающие пробелы
/// (текст приходит кусками, обрезать края нельзя — склеились бы слова).
fn rewrite_urls(text: &str, loc: &Locale) -> String {
    if !text.contains("://") {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    // `split_inclusive` сохраняет хвостовой пробельный символ каждого куска.
    for piece in text.split_inclusive(char::is_whitespace) {
        let core = piece.trim_end();
        out.push_str(&rewrite_token(core, loc));
        out.push_str(&piece[core.len()..]);
    }
    out
}

/// Заменяет один токен, если он — http(s)-URL (обрамляющая пунктуация сохраняется).
fn rewrite_token(token: &str, loc: &Locale) -> String {
    const LEAD: [char; 4] = ['(', '[', '«', '"'];
    const TRAIL: [char; 9] = [')', ']', '»', '"', '.', ',', ';', '!', '?'];
    let lead_len = token.len() - token.trim_start_matches(LEAD).len();
    let (lead, rest) = token.split_at(lead_len);
    let core_len = rest.trim_end_matches(TRAIL).len();
    let (core, trail) = rest.split_at(core_len);
    let lower = core.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return token.to_string();
    }
    let domain = domain_of(core);
    let spoken = loc.tf("speak.link", &[("domain", &domain)]);
    format!("{lead}{spoken}{trail}")
}

/// Домен URL: без схемы, пути, порта и префикса `www.`.
fn domain_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let host = rest
        .split(['/', '?', '#', ':'])
        .next()
        .unwrap_or(rest)
        .trim_start_matches("www.");
    host.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::i18n::{Lang, locale};

    fn ru() -> &'static Locale {
        locale(Lang::Ru)
    }

    fn speak(md: &str) -> String {
        speakable_text(md, ru())
    }

    /// Golden: характерный ответ LLM — проза + код + mermaid + таблица + формулы +
    /// ссылки + список + заголовок. Проверяем ВЕСЬ вывод целиком.
    #[test]
    fn golden_typical_llm_answer() {
        let md = "\
# Отчёт

Сложность равна $O(n \\log n)$, см. `sort()`.

```rust
fn main() {}
```

```mermaid
flowchart LR
    A --> B
```

| A | B |
| :--- | :--- |
| 1 | 2 |

$$E = mc^2$$

Подробности — [в документации](https://docs.example.com/guide) и на https://example.org/page.

- первый пункт
- второй пункт";
        let got = speak(md);
        let expected = "\
Отчёт.
Сложность равна O(n log n), см. sort().
(блок кода пропущен).
(диаграмма пропущена).
(таблица пропущена).
(формула пропущена).
Подробности — в документации и на ссылка: example.org.
первый пункт.
второй пункт.";
        assert_eq!(got, expected);
    }

    /// Пер-локальный гейт: тот же текст на английском не содержит кириллицы и
    /// отличается от русского (ловит забытую локализацию пометок).
    #[test]
    fn english_output_has_no_cyrillic() {
        let md = "Text.\n\n```rust\nfn main() {}\n```\n\n| A |\n| - |\n| 1 |\n\n$$x$$\n\n![](i.png)\n\nhttps://example.com";
        let en = speakable_text(md, locale(Lang::En));
        let ru_out = speak(md);
        assert_ne!(en, ru_out, "пометки не локализованы");
        assert!(
            !en.chars()
                .any(|c| matches!(c, 'а'..='я' | 'А'..='Я' | 'ё' | 'Ё')),
            "кириллица в английском выводе: {en}"
        );
        assert!(en.contains("(code block skipped)"), "{en}");
        assert!(en.contains("(table skipped)"), "{en}");
        assert!(en.contains("(formula skipped)"), "{en}");
        assert!(en.contains("(image)"), "{en}");
        assert!(en.contains("link: example.com"), "{en}");
    }

    #[test]
    fn empty_input_gives_empty_output() {
        assert_eq!(speak(""), "");
        assert_eq!(speak("   \n\n\t "), "");
    }

    /// Сообщение из одного код-блока — только пометка (озвучивать нечего).
    #[test]
    fn only_code_block_gives_note() {
        assert_eq!(
            speak("```python\nprint('hi')\n```"),
            "(блок кода пропущен)."
        );
        // Блок с отступом (indented) — тоже код.
        assert_eq!(speak("    let x = 1;"), "(блок кода пропущен).");
    }

    /// Незакрытый забор (обрыв стрима) — содержимое всё равно не читается.
    #[test]
    fn unclosed_fence_is_skipped() {
        let got = speak("Начало.\n\n```rust\nfn main() {\n");
        assert_eq!(got, "Начало.\n(блок кода пропущен).");
    }

    /// Вложенные списки: каждый пункт — своя фраза, маркеры не озвучиваются.
    #[test]
    fn nested_lists_speak_item_by_item() {
        let got = speak("- один\n  - вложенный\n- два");
        assert_eq!(got, "один.\nвложенный.\nдва.");
    }

    /// Автолинк `<url>` и голый URL читаются доменом; `www.` отбрасывается.
    #[test]
    fn autolink_and_bare_url_read_as_domain() {
        assert_eq!(
            speak("см. <https://www.example.com/a/b?q=1> тут"),
            "см. ссылка: example.com тут."
        );
        assert_eq!(
            speak("сайт https://example.org, потом текст"),
            "сайт ссылка: example.org, потом текст."
        );
    }

    /// У ссылки читается только текст, URL опускается.
    #[test]
    fn link_reads_text_without_url() {
        assert_eq!(
            speak("открой [документацию](https://docs.example.com)"),
            "открой документацию."
        );
    }

    /// Изображение: alt читается, без alt — пометка.
    #[test]
    fn image_reads_alt_or_note() {
        assert_eq!(speak("![схема сети](i.png)"), "схема сети.");
        assert_eq!(speak("![](i.png)"), "(изображение).");
    }

    /// Inline-код внутри предложения читается текстом и не рвёт фразу.
    #[test]
    fn inline_code_stays_in_sentence() {
        assert_eq!(
            speak("вызови `note_save` перед выходом"),
            "вызови note_save перед выходом."
        );
    }

    /// Заголовки и цитаты — отдельными фразами с паузой.
    #[test]
    fn headings_and_quotes_are_separate_phrases() {
        assert_eq!(
            speak("## Итог\n\n> цитата без точки\n\nконец!"),
            "Итог.\nцитата без точки.\nконец!"
        );
    }

    /// Inline-математика конвертируется, диапазон цен не съедается math-расширением.
    #[test]
    fn inline_math_and_price_range() {
        assert_eq!(speak("формула $x^2$ тут"), "формула x² тут.");
        assert_eq!(speak("товар $5-$10 сегодня"), "товар $5-$10 сегодня.");
    }

    /// Эмодзи остаются как есть (движки читают названием или игнорируют).
    #[test]
    fn emoji_pass_through() {
        assert_eq!(speak("готово 🎉"), "готово 🎉.");
    }

    /// Многострочный абзац (мягкие переносы) склеивается в одну фразу без
    /// висячих пробелов и двойных пропусков.
    #[test]
    fn soft_breaks_collapse_to_single_spaces() {
        assert_eq!(
            speak("первая\nвторая\n\n\nтретья"),
            "первая вторая.\nтретья."
        );
    }

    /// Домен вычисляется без схемы/пути/порта.
    #[test]
    fn domain_extraction() {
        assert_eq!(domain_of("https://example.com/a?b=1#c"), "example.com");
        assert_eq!(domain_of("http://www.example.com:8080/x"), "example.com");
    }
}
