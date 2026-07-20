//! Лента сообщений: markdown-рендер тел (через [`crate::shared::markdown`]),
//! сворачиваемый блок «мыслей» (CoT) и вертикальный скролл. См. spec §11.3–11.4.
//!
//! «Мысли» сворачиваются глобальным переключателем (`show_thoughts`); tool-блоки
//! (имя/аргументы/результат) показываются внутри сообщения ассистента (M5).
//! Выделение отдельных сообщений/блоков — позже. Виджет хранит только состояние
//! просмотра (скролл, «следовать за хвостом», показ мыслей); сами сообщения
//! принадлежат UI-состоянию и передаются на отрисовку.

use ratatui::Frame;
use ratatui::layout::{Margin, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::entities::message::{Message, MessageRole};
use crate::features::tools::present::{self, ToolBlock};
use crate::shared::i18n::Locale;
use crate::shared::markdown;
use crate::shared::theme::Palette;
use crate::shared::ui::render_scrollbar;
use crate::shared::wrap;

/// Гуттер-рейл слева от каждой строки сообщения: цветная вертикальная черта +
/// пробел (редизайн: «Role rails — цветные ▌ в гаттере»). Ширина — 2 колонки.
const RAIL: &str = "▌ ";

/// Роль элемента ленты (UI-проекция; системные сообщения не показываются).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedRole {
    User,
    Assistant,
    /// Служебная заметка (ошибки, «генерация отменена»).
    Note,
}

/// Tool-блок в ленте: имя инструмента, аргументы и результат. См. spec §11.3.
///
/// `text_offset` — позиция вызова **в байтах** внутри `FeedMessage::text`: сколько
/// текста ответа ассистента было сгенерировано ДО этого вызова. Так tool-блок
/// рисуется ровно на месте вызова (между фрагментами текста), а не в «шапке».
#[derive(Debug, Clone)]
pub struct FeedToolCall {
    pub name: String,
    pub arguments: String,
    pub result: String,
    /// Смещение (в байтах) в `FeedMessage::text`, после которого был сделан вызов.
    pub text_offset: usize,
}

/// Элемент ленты сообщений.
#[derive(Debug, Clone)]
pub struct FeedMessage {
    pub role: FeedRole,
    pub text: String,
    pub thoughts: String,
    /// Вызовы инструментов этого сообщения ассистента (tool-блоки).
    pub tools: Vec<FeedToolCall>,
    /// Идёт ли стриминг этого сообщения (показываем «…» вместо пустого тела).
    pub streaming: bool,
}

impl FeedMessage {
    /// Служебная заметка для ленты.
    pub fn note(text: impl Into<String>) -> Self {
        Self {
            role: FeedRole::Note,
            text: text.into(),
            thoughts: String::new(),
            tools: Vec::new(),
            streaming: false,
        }
    }

    /// Проекция доменного сообщения (для перестроения ленты при активации чата).
    /// Системные и tool-сообщения отбрасываются (`None`): tool-вызовы показываются
    /// как блоки внутри сообщения ассистента (`tool_calls`), а не отдельно.
    ///
    /// Все вызовы инструментов раунда сделаны ПОСЛЕ его текста, поэтому их
    /// `text_offset` = длина текста (вызовы рисуются под ним).
    pub fn from_message(msg: &Message) -> Option<Self> {
        let role = match msg.role {
            MessageRole::User => FeedRole::User,
            MessageRole::Assistant => FeedRole::Assistant,
            MessageRole::Tool | MessageRole::System => return None,
        };
        let off = msg.text.len();
        // Управляющие инструменты беседы (followup/rewrite) — служебные сигналы,
        // не контент: их tool-блоки в ленте не показываем. См. spec §9.3.
        let tools = msg
            .tool_calls
            .iter()
            .filter(|tc| !crate::features::tools::control::is_control_tool(&tc.name))
            .map(|tc| FeedToolCall {
                name: tc.name.clone(),
                arguments: tc.arguments.to_string(),
                result: tc.result.clone().unwrap_or_default(),
                text_offset: off,
            })
            .collect();
        Some(Self {
            role,
            text: msg.text.clone(),
            thoughts: msg.thoughts.clone().unwrap_or_default(),
            tools,
            streaming: false,
        })
    }

    /// Проекция списка доменных сообщений в ленту со **склейкой раундов**: подряд
    /// идущие сообщения ассистента (раунды agentic-loop, между которыми в истории
    /// лежат tool-сообщения — они отбрасываются) сливаются в один блок «Ассистент:».
    /// Тексты раундов конкатенируются (через пустую строку), а `text_offset`
    /// вызовов каждого раунда сдвигается на накопленную длину — так live-стрим и
    /// перезагрузка из истории дают одинаковую инлайн-раскладку вызовов.
    pub fn from_messages(messages: &[Message]) -> Vec<Self> {
        let mut out: Vec<Self> = Vec::new();
        for msg in messages {
            let Some(mut fm) = Self::from_message(msg) else {
                continue;
            };
            // Сливаем раунд ассистента с предыдущим блоком ассистента — кроме
            // случая, когда сообщение помечено `new_bubble` (инструмент «написать
            // ещё сообщение»): тогда оно начинает отдельный пузырь. См. spec §9.3.
            if fm.role == FeedRole::Assistant
                && !msg.new_bubble
                && let Some(last) = out.last_mut()
                && last.role == FeedRole::Assistant
            {
                if !fm.text.is_empty() {
                    if !last.text.is_empty() {
                        last.text.push_str("\n\n");
                    }
                    last.text.push_str(&fm.text);
                }
                let base = last.text.len();
                for mut tc in fm.tools.drain(..) {
                    tc.text_offset = base;
                    last.tools.push(tc);
                }
                if !fm.thoughts.is_empty() {
                    if !last.thoughts.is_empty() {
                        last.thoughts.push('\n');
                    }
                    last.thoughts.push_str(&fm.thoughts);
                }
                continue;
            }
            out.push(fm);
        }
        out
    }
}

/// Состояние просмотра ленты (скролл, показ мыслей).
pub struct MessageFeed {
    /// Смещение прокрутки (в строках от начала).
    scroll: usize,
    /// Следовать за хвостом (автопрокрутка к низу при новом контенте).
    follow: bool,
    /// Показывать развёрнутый блок «мыслей».
    show_thoughts: bool,
    /// Флаг «лента только что прокручена пользователем» — петля по нему делает
    /// полную перерисовку терминала (как при ресайзе). Нужен из-за артефактов
    /// некоторых терминалов (Command Prompt/conhost) на VS16-эмодзи (`🕸️`,
    /// `🗂️`): они рисуют такой кластер физически шире модели ratatui, контент
    /// «съезжает» по горизонтали, и при странично-скачковой прокрутке (PageUp/
    /// PageDown) на месте уехавшего символа остаётся «висячая» буква в физической
    /// ячейке, которую поячеечный diff ratatui больше не затрагивает. Полная
    /// перерисовка (`terminal.clear`) гарантированно её стирает. См. spec §11.3.
    scrolled: bool,
    /// Флаг «содержимое перерисовано заново» (промах кэша блока при последнем
    /// рендере). Забирается экраном — см. [`MessageFeed::take_content_changed`].
    content_changed: bool,
    /// Горизонтальные разделители между строками Markdown-таблиц (настройка
    /// `interface.table_row_separators`; экран чата прокидывает её из снимка
    /// настроек через [`MessageFeed::set_table_row_separators`]).
    table_row_separators: bool,
    /// Рендерить ```mermaid-блоки диаграммой (настройка `interface.render_mermaid`,
    /// прокидывается через [`MessageFeed::set_render_mermaid`]). При сбое рендера
    /// блок печатается исходником (жёсткий фолбэк, см. `shared::markdown::mermaid`).
    render_mermaid: bool,
    /// Кэш отрендеренных строк по одному блоку на сообщение (см. [`CachedBlock`]).
    /// Индекс = позиция сообщения. `build_lines` зовётся на каждый dirty-кадр
    /// (стрим, прокрутка) и заново прогонял бы markdown+syntect по ВСЕЙ истории;
    /// кэш пересчитывает лишь изменившиеся сообщения (сверка по фингерпринту).
    cache: Vec<CachedBlock>,
    /// Ключ кэша: при смене ширины/палитры/показа мыслей кэш сбрасывается целиком.
    cache_key: Option<CacheKey>,
}

/// Ключ валидности кэша ленты. Любое из полей влияет на раскладку всех блоков,
/// поэтому его смена обнуляет кэш. `Palette` — `Copy + Eq` (ключ и в кэше syntect-тем).
#[derive(PartialEq)]
struct CacheKey {
    width: usize,
    palette: Palette,
    show_thoughts: bool,
    table_row_separators: bool,
    render_mermaid: bool,
    /// Язык интерфейса (ось B): заголовки ролей/пилюля «мысли»/плейсхолдер зависят
    /// от него — смена языка обнуляет кэш блоков ленты.
    lang: crate::shared::i18n::Lang,
}

/// Кэшированный вклад одного сообщения в ленту (уже перенесённые по ширине строки с
/// рейлом + хвостовой разделитель) вместе с фингерпринтом исходного [`FeedMessage`].
struct CachedBlock {
    fingerprint: u64,
    lines: Vec<Line<'static>>,
}

impl Default for MessageFeed {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageFeed {
    pub fn new() -> Self {
        Self {
            scroll: 0,
            follow: true,
            show_thoughts: false,
            scrolled: false,
            content_changed: false,
            // Зеркало дефолтов конфига (`InterfaceSettings::default`): до прихода
            // первого снимка настроек лента рисует как дефолтный конфиг.
            table_row_separators: false,
            render_mermaid: true,
            cache: Vec::new(),
            cache_key: None,
        }
    }

    /// Включает/выключает горизонтальные разделители строк Markdown-таблиц
    /// (настройка `interface.table_row_separators`). Смена значения инвалидирует
    /// кэш рендера через [`CacheKey`].
    pub fn set_table_row_separators(&mut self, on: bool) {
        self.table_row_separators = on;
    }

    /// Включает/выключает рендер ```mermaid-блоков диаграммой (настройка
    /// `interface.render_mermaid`). Смена значения инвалидирует кэш через [`CacheKey`].
    pub fn set_render_mermaid(&mut self, on: bool) {
        self.render_mermaid = on;
    }

    /// Забирает (и сбрасывает) флаг «содержимое ленты перерисовано заново» (промах
    /// кэша блока: стрим, новая заметка, правка истории). Экран по нему заказывает
    /// полную перерисовку терминала, когда в ленте есть глифы из группы риска —
    /// иначе на legacy-терминалах остаются артефакты от изменившихся строк с эмодзи
    /// (прокрутка их «чинила», потому что была единственным триггером). См. spec §11.3.
    pub fn take_content_changed(&mut self) -> bool {
        std::mem::take(&mut self.content_changed)
    }

    /// Забирает (и сбрасывает) флаг «прокручено пользователем». Петля `app/runtime`
    /// при `true` делает `terminal.clear()` перед отрисовкой — стирает артефакты
    /// «съехавших» VS16-эмодзи (см. поле [`MessageFeed::scrolled`]).
    pub fn take_scrolled(&mut self) -> bool {
        std::mem::take(&mut self.scrolled)
    }

    /// Переключает показ блоков «мыслей».
    pub fn toggle_thoughts(&mut self) {
        self.show_thoughts = !self.show_thoughts;
    }

    /// Прокрутка вверх (отключает «следование за хвостом»).
    pub fn scroll_up(&mut self, lines: usize) {
        self.scroll = self.scroll.saturating_sub(lines);
        self.follow = false;
        self.scrolled = true;
    }

    /// Прокрутка вниз (у самого низа снова включает «следование»).
    pub fn scroll_down(&mut self, lines: usize) {
        self.scroll = self.scroll.saturating_add(lines);
        self.scrolled = true;
        // Фактический кламп и повторное включение follow — в render (там известна высота).
    }

    /// Сбрасывает прокрутку к низу (при смене чата/отправке).
    pub fn scroll_to_bottom(&mut self) {
        self.follow = true;
    }

    /// Тест-аксессор: следует ли лента за хвостом (прокрутка к низу).
    #[cfg(test)]
    pub(crate) fn is_following(&self) -> bool {
        self.follow
    }

    /// Рисует ленту. `messages` — текущее содержимое активного чата. `meta` —
    /// правая подпись титула (напр. «gemma-4 · 16k ctx»; пусто — не показывать).
    // Титул/мета/палитра/локаль — контекст отрисовки; собирать их в struct ради
    // одного вызова из `chat/render.rs` не окупается.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        title: &str,
        meta: &str,
        messages: &[FeedMessage],
        palette: &Palette,
        loc: &'static Locale,
    ) {
        // Скруглённая панель: слева титул с маркером ◆, справа — мета (модель/ctx).
        let glyphs = palette.glyphs();
        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_type(glyphs.border)
            .border_style(palette.border_style(false))
            .title(Line::from(vec![
                Span::styled(format!(" {} ", glyphs.title_marker), palette.muted_style()),
                Span::styled(format!("{title} "), Style::new().fg(palette.text)),
            ]));
        if !meta.is_empty() {
            block = block.title(
                Line::from(Span::styled(format!(" {meta} "), palette.muted_style()))
                    .right_aligned(),
            );
        }
        let inner = block.inner(area);
        frame.render_widget(&block, area);

        // Переносим строки по ширине ленты заранее: так число визуальных рядов
        // совпадает с `lines.len()`, и математика скролла/«следования за хвостом»
        // ниже остаётся row-based (см. shared::wrap, ADR 0001).
        let view_w = inner.width.max(1) as usize;
        let lines: Vec<Line> = self
            .build_lines(messages, palette, view_w, loc)
            .iter()
            .flat_map(|l| wrap::wrap_line(l, view_w))
            .collect();
        let total = lines.len();
        let view_h = inner.height.max(1) as usize;
        let max_scroll = total.saturating_sub(view_h);

        if self.follow {
            self.scroll = max_scroll;
        } else if self.scroll >= max_scroll {
            // докрутили до низа — снова следуем за хвостом
            self.scroll = max_scroll;
            self.follow = true;
        }

        let paragraph = Paragraph::new(Text::from(lines)).scroll((self.scroll as u16, 0));
        frame.render_widget(paragraph, inner);

        // Скроллбар на правой рамке панели (углы не трогаем) — только когда
        // лента не помещается по высоте. Ширину содержимого не отнимает; рамка
        // ленты всегда не-фокусная (см. `border_style(false)` выше).
        render_scrollbar(
            frame,
            area.inner(Margin::new(0, 1)),
            total,
            view_h,
            self.scroll,
            false,
            palette,
        );
    }

    /// Собирает строки ленты: заголовки ролей, свёрнутые/развёрнутые «мысли»,
    /// markdown-рендер тела, разделители. Каждая строка сообщения получает цветной
    /// гуттер-рейл по роли (см. [`RAIL`]); содержимое строится в ширину `width - 2`,
    /// затем переносится и к каждому визуальному ряду прикрепляется рейл.
    fn build_lines(
        &mut self,
        messages: &[FeedMessage],
        palette: &Palette,
        width: usize,
        loc: &'static Locale,
    ) -> Vec<Line<'static>> {
        if messages.is_empty() {
            // Плейсхолдер пустой ленты не кэшируем.
            return vec![Line::from(Span::styled(
                loc.t("ui.feed.empty").to_string(),
                palette.muted_style(),
            ))];
        }
        // Сброс кэша при смене ширины/палитры/показа мыслей/языка (влияют на блоки).
        let key = CacheKey {
            width,
            palette: *palette,
            show_thoughts: self.show_thoughts,
            table_row_separators: self.table_row_separators,
            render_mermaid: self.render_mermaid,
            lang: loc.lang(),
        };
        if self.cache_key.as_ref() != Some(&key) {
            self.cache.clear();
            self.cache_key = Some(key);
        }
        // История усечена (Ctrl+E/regenerate) — отбрасываем хвост кэша.
        self.cache.truncate(messages.len());

        // Базовые флаги markdown-рендера из настроек ленты; `soft_break_as_newline`
        // остаётся per-role (его ставит push_body для сообщений пользователя).
        let opts = markdown::RenderOpts {
            table_row_separators: self.table_row_separators,
            render_mermaid: self.render_mermaid,
            ..Default::default()
        };
        let mut lines: Vec<Line<'static>> = Vec::new();
        for (idx, item) in messages.iter().enumerate() {
            let fp = message_fingerprint(item);
            let hit = self.cache.get(idx).is_some_and(|c| c.fingerprint == fp);
            if !hit {
                // Содержимое блока изменилось (стрим, новая заметка, правка) — экран
                // по этому флагу закажет полную перерисовку, если в ленте есть глифы
                // из группы риска. См. [`Self::take_content_changed`].
                self.content_changed = true;
                // Стримящееся/изменённое сообщение — пересчитываем только его блок.
                let block =
                    build_message_block(item, palette, width, self.show_thoughts, opts, loc);
                let cb = CachedBlock {
                    fingerprint: fp,
                    lines: block,
                };
                if idx < self.cache.len() {
                    self.cache[idx] = cb;
                } else {
                    self.cache.push(cb);
                }
            }
            lines.extend(self.cache[idx].lines.iter().cloned());
        }
        lines
    }
}

/// Собирает вклад одного сообщения в ленту: перенесённые по ширине строки с цветным
/// рейлом роли + хвостовой разделитель (если тело не оканчивается пустой строкой).
/// Чистая функция от (`item`, `palette`, `width`, `show_thoughts`, `opts`) —
/// основа кэша. `opts` — базовые флаги markdown-рендера (таблицы/mermaid из
/// настроек; `soft_break_as_newline` докидывает `push_body` для пользователя).
fn build_message_block(
    item: &FeedMessage,
    palette: &Palette,
    width: usize,
    show_thoughts: bool,
    opts: markdown::RenderOpts,
    loc: &'static Locale,
) -> Vec<Line<'static>> {
    // Ширина содержимого под рейл (рейл = 2 колонки).
    let inner = width.saturating_sub(RAIL.chars().count()).max(1);
    let rail = match item.role {
        FeedRole::User => palette.user,
        FeedRole::Assistant => palette.assistant,
        FeedRole::Note => palette.muted,
    };
    // Тело сообщения собираем без рейла, в ширину `inner`.
    let mut body: Vec<Line<'static>> = Vec::new();
    let glyphs = palette.glyphs();
    match item.role {
        FeedRole::User => {
            body.push(role_header(
                &format!("{} {}", glyphs.user_icon, loc.t("ui.feed.role.user")),
                palette.user_soft,
            ));
            push_body(&mut body, item, palette, inner, opts);
        }
        FeedRole::Assistant => {
            body.push(role_header(
                &format!(
                    "{} {}",
                    glyphs.assistant_icon,
                    loc.t("ui.feed.role.assistant")
                ),
                palette.assistant_soft,
            ));
            push_thoughts(&mut body, &item.thoughts, show_thoughts, palette, loc);
            push_assistant_body(&mut body, item, palette, inner, opts);
        }
        FeedRole::Note => push_body(&mut body, item, palette, inner, opts),
    }
    // Если тело уже заканчивается пустой строкой (рейловый отступ после tool-карточки),
    // безрейловый межсообщенческий разделитель не добавляем — иначе двойной пропуск.
    let body_ends_blank = body
        .last()
        .map(|l| l.spans.iter().all(|s| s.content.trim().is_empty()))
        .unwrap_or(false);
    // Переносим по ширине содержимого и навешиваем рейл на каждый ряд.
    let mut out: Vec<Line<'static>> = Vec::new();
    for line in body {
        for wrapped in wrap::wrap_line(&line, inner) {
            out.push(prepend_rail(wrapped, rail));
        }
    }
    // Разделитель между сообщениями — без рейла.
    if !body_ends_blank {
        out.push(Line::from(""));
    }
    out
}

/// Фингерпринт сообщения по всем полям, влияющим на рендер. Хеш O(len) против
/// рендера O(len·markdown+syntect) — на порядки дешевле; стримящееся сообщение меняет
/// `text` каждым чанком → фингерпринт не совпадает → пересчитывается только оно.
fn message_fingerprint(item: &FeedMessage) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let role_tag: u8 = match item.role {
        FeedRole::User => 0,
        FeedRole::Assistant => 1,
        FeedRole::Note => 2,
    };
    role_tag.hash(&mut h);
    item.text.hash(&mut h);
    item.thoughts.hash(&mut h);
    item.streaming.hash(&mut h);
    item.tools.len().hash(&mut h);
    for tc in &item.tools {
        tc.name.hash(&mut h);
        tc.arguments.hash(&mut h);
        tc.result.hash(&mut h);
        tc.text_offset.hash(&mut h);
    }
    h.finish()
}

/// Строка-заголовок роли: иконка + название капсом, цветом «мягкого» варианта роли.
fn role_header(text: &str, color: Color) -> Line<'static> {
    Line::from(Span::styled(
        text.to_string(),
        Style::new().fg(color).add_modifier(Modifier::BOLD),
    ))
}

/// Прикрепляет цветной гуттер-рейл [`RAIL`] к строке (в начало), сохраняя стиль и
/// выравнивание исходной строки.
///
/// Стиль уровня строки (`line.style`) **вплавляется в спаны содержимого**, а
/// `out.style` сбрасывается в дефолт — иначе line-level модификаторы (напр. `DIM`
/// у разделителей `───` и рамок таблиц, см. `shared::markdown`) затекали бы и на
/// сам рейл (его спан задаёт только `fg`, не трогая модификаторы), и он выглядел бы
/// другим цветом напротив таких строк.
fn prepend_rail(line: Line<'static>, rail: Color) -> Line<'static> {
    let mut spans = Vec::with_capacity(line.spans.len() + 1);
    spans.push(Span::styled(RAIL.to_string(), Style::new().fg(rail)));
    // Финальный стиль спана = line.style.patch(span.style); складываем то же самое в
    // сам спан, чтобы рейл не зависел от стиля строки.
    for span in line.spans {
        let merged = line.style.patch(span.style);
        spans.push(Span::styled(span.content, merged));
    }
    let mut out = Line::from(spans);
    out.alignment = line.alignment;
    out
}

/// Добавляет блок «мыслей»: свёрнутый — «пилюлей» с числом строк и клавишей,
/// развёрнутый — содержимым на гуттере `│`.
fn push_thoughts(
    lines: &mut Vec<Line<'static>>,
    thoughts: &str,
    expanded: bool,
    palette: &Palette,
    loc: &'static Locale,
) {
    if thoughts.is_empty() {
        return;
    }
    let muted = palette.muted_style();
    let glyphs = palette.glyphs();
    if !expanded {
        let count = thoughts.lines().count();
        lines.push(Line::from(vec![
            Span::styled(format!("{} ", glyphs.collapsed), muted),
            Span::styled(
                loc.t("ui.feed.thoughts").to_string(),
                muted.add_modifier(Modifier::ITALIC),
            ),
            Span::styled(
                loc.tf("ui.feed.thoughts_lines", &[("n", &count.to_string())]),
                muted,
            ),
            palette.keycap("Ctrl+T"),
        ]));
        return;
    }
    lines.push(Line::from(Span::styled(
        format!("{} мысли", glyphs.expanded),
        muted.add_modifier(Modifier::ITALIC),
    )));
    for t in thoughts.lines() {
        lines.push(Line::from(Span::styled(
            format!("│ {t}"),
            muted.add_modifier(Modifier::ITALIC),
        )));
    }
}

/// Добавляет тело ответа ассистента с tool-блоками **на местах вызова**: фрагмент
/// текста до вызова → tool-блок → следующий фрагмент и т.д. (см. spec §11.3).
/// `text_offset` каждого вызова делит `item.text` на фрагменты markdown.
///
/// Tool-блоки отделяются от текста (и от соседних tool-блоков) пустой строкой
/// сверху и снизу, чтобы не сливались с сообщением; идущие подряд вызовы делит
/// ровно одна пустая строка (`ensure_blank_line` схлопывает соседние).
fn push_assistant_body(
    lines: &mut Vec<Line<'static>>,
    item: &FeedMessage,
    palette: &Palette,
    width: usize,
    opts: markdown::RenderOpts,
) {
    let text = item.text.as_str();
    let mut pos = 0usize;
    let mut produced = false;
    for tool in &item.tools {
        let off = clamp_boundary(text, tool.text_offset.min(text.len())).max(pos);
        if off > pos {
            push_markdown_fragment(lines, &text[pos..off], palette, width, opts);
        }
        // Пустая строка перед вызовом (схлопывается, если предыдущая уже пуста —
        // напр. между двумя подряд идущими вызовами).
        ensure_blank_line(lines);
        push_tool(lines, tool, palette, width, opts);
        // Пустая (рейловая) строка ПОСЛЕ карточки — чтобы рейл продолжался под
        // результатом независимо от того, идёт ли дальше текст/ещё вызов. Соседние
        // `ensure_blank_line` схлопываются (перед следующим вызовом/текстом — no-op),
        // а на конце сообщения этот отступ заменяет межсообщенческий разделитель
        // (см. `build_lines`). Раньше отступ после последнего вызова давал лишь
        // безрейловый разделитель, и рейл обрывался на результате — заметно у
        // `python_exec`, чей результат часто и есть финал хода.
        ensure_blank_line(lines);
        produced = true;
        pos = off;
    }
    if pos < text.len() && push_markdown_fragment(lines, &text[pos..], palette, width, opts) {
        produced = true;
    }
    // Пустой стримящийся ответ (ещё ни текста, ни вызовов) — индикатор «…».
    if !produced && item.streaming {
        lines.push(Line::from("…").dim());
    }
}

/// Добавляет пустую строку-разделитель, если последняя строка ещё не пуста.
/// Так соседние разделители (напр. «после вызова» + «перед следующим вызовом»)
/// схлопываются в одну пустую строку. На пустом буфере — no-op (без ведущей пустой).
fn ensure_blank_line(lines: &mut Vec<Line<'static>>) {
    let blank = lines
        .last()
        .map(|l| l.spans.iter().all(|s| s.content.trim().is_empty()))
        .unwrap_or(true);
    if !blank {
        lines.push(Line::from(""));
    }
}

/// Рендерит фрагмент текста как markdown (если он не пустой). Возвращает, выдал ли строки.
fn push_markdown_fragment(
    lines: &mut Vec<Line<'static>>,
    fragment: &str,
    palette: &Palette,
    width: usize,
    opts: markdown::RenderOpts,
) -> bool {
    if fragment.trim().is_empty() {
        return false;
    }
    let rendered = markdown::render_with(fragment, width, palette, opts);
    lines.extend(rendered.lines);
    true
}

/// Один tool-блок (карточка тул-колла): заголовок `⚒ имя(суффикс)` цветом
/// инструмента, затем блоки аргументов и результата, подготовленные презентером
/// [`present`] (подсвеченный код, консольный вывод, markdown-проза, плоский
/// текст). Всё **переносится по ширине** (не обрезается). См. spec §11.3.
fn push_tool(
    lines: &mut Vec<Line<'static>>,
    tool: &FeedToolCall,
    palette: &Palette,
    width: usize,
    opts: markdown::RenderOpts,
) {
    let head_style = Style::default()
        .fg(palette.tool_soft)
        .add_modifier(Modifier::BOLD);
    let p = present::present(&tool.name, &tool.arguments, &tool.result);
    let header = match &p.header_suffix {
        Some(suffix) => format!("{}({suffix})", tool.name),
        None => tool.name.clone(),
    };
    // Первый ряд с иконкой ⚒ (эмодзи-глиф шириной 2 — за ним два пробела, чтобы он
    // не сливался с именем), продолжения выравниваем под имя. В режиме
    // совместимости — ASCII-префикс той же роли (см. GlyphSet::tool_head).
    let glyphs = palette.glyphs();
    push_wrapped(
        lines,
        glyphs.tool_head,
        glyphs.tool_cont,
        &header,
        width,
        head_style,
    );
    for block in &p.args {
        push_block(lines, block, palette, width, false, opts);
    }
    for block in &p.result {
        push_block(lines, block, palette, width, true, opts);
    }
}

/// Рисует один блок tool-карточки. `is_result` меняет ведущий гуттер: результат
/// начинается с `└ ` (углом), аргумент — с `│ ` (вертикальной чертой, «вложен под
/// заголовок»). Гуттер `│`/`└` — WGL4-безопасен и в режиме совместимости.
fn push_block(
    lines: &mut Vec<Line<'static>>,
    block: &ToolBlock,
    palette: &Palette,
    width: usize,
    is_result: bool,
    opts: markdown::RenderOpts,
) {
    let gutter_style = Style::default().fg(palette.muted);
    match block {
        ToolBlock::Plain(text) => {
            let (first, cont) = if is_result {
                ("└ ", "  ")
            } else {
                ("│ ", "│ ")
            };
            push_wrapped(lines, first, cont, text, width, gutter_style);
        }
        ToolBlock::Code { lang, text } => {
            let hl = markdown::highlight_code(text, lang, palette);
            push_gutter_lines(lines, hl, "│ ", "│ ", gutter_style, width);
        }
        ToolBlock::Markdown(text) => {
            let body_w = width.saturating_sub(2).max(1);
            let rendered = markdown::render_with(text, body_w, palette, opts);
            push_gutter_lines(lines, rendered.lines, "└ ", "  ", gutter_style, width);
        }
        ToolBlock::Console(c) => push_console(lines, c, palette, width),
    }
}

/// Рисует консольный вывод `python_exec`: stdout (цветом текста), stderr (цветом
/// ошибки) и код возврата (цветом предупреждения) отдельными секциями. Каждая
/// секция — метка на гуттере `└ ` и содержимое на `│ `.
fn push_console(
    lines: &mut Vec<Line<'static>>,
    console: &present::Console,
    palette: &Palette,
    width: usize,
) {
    let label = Style::default().fg(palette.muted);
    let out_style = Style::default().fg(palette.text);
    let err_style = Style::default().fg(palette.error);
    if !console.stdout.trim().is_empty() {
        push_wrapped(lines, "└ ", "  ", "stdout", width, label);
        push_wrapped(lines, "│ ", "│ ", &console.stdout, width, out_style);
    }
    if !console.stderr.trim().is_empty() {
        push_wrapped(lines, "└ ", "  ", "stderr", width, err_style);
        push_wrapped(lines, "│ ", "│ ", &console.stderr, width, err_style);
    }
    if let Some(code) = console.exit {
        let warn = Style::default().fg(palette.warning);
        push_wrapped(
            lines,
            "└ ",
            "  ",
            &format!("код возврата: {code}"),
            width,
            warn,
        );
    }
}

/// Кладёт готовые (уже стилизованные) строки под гуттер-префиксами, перенося
/// каждую по ширине. `first`/`cont` — префиксы первого/последующих рядов. Стиль
/// уровня строки вплавляется в спаны содержимого (как в [`prepend_rail`]), чтобы
/// line-level модификаторы (напр. `DIM` рамок таблиц) не затекали на гуттер.
fn push_gutter_lines(
    lines: &mut Vec<Line<'static>>,
    src: Vec<Line<'static>>,
    first: &str,
    cont: &str,
    gutter_style: Style,
    width: usize,
) {
    let first_w = wrap::display_width(&first.chars().collect::<Vec<_>>());
    let cont_w = wrap::display_width(&cont.chars().collect::<Vec<_>>());
    let body_w = width.saturating_sub(first_w.max(cont_w)).max(1);
    let mut first_row = true;
    for line in src {
        let line_style = line.style;
        let align = line.alignment;
        for wrapped in wrap::wrap_line(&line, body_w) {
            let prefix = if first_row { first } else { cont };
            first_row = false;
            let mut spans = Vec::with_capacity(wrapped.spans.len() + 1);
            spans.push(Span::styled(prefix.to_string(), gutter_style));
            for span in wrapped.spans {
                spans.push(Span::styled(span.content, line_style.patch(span.style)));
            }
            let mut out = Line::from(spans);
            out.alignment = align;
            lines.push(out);
        }
    }
}

/// Переносит `text` по визуальной ширине с гуттер-префиксами и кладёт в `lines`.
/// `first` — префикс самого первого ряда блока, `cont` — всех последующих рядов
/// (и продолжений переноса, и новых логических строк исходника). Каждая строка
/// `text` переносится отдельно, сохраняя переводы строк.
fn push_wrapped(
    lines: &mut Vec<Line<'static>>,
    first: &str,
    cont: &str,
    text: &str,
    width: usize,
    style: Style,
) {
    let first_w = wrap::display_width(&first.chars().collect::<Vec<_>>());
    let cont_w = wrap::display_width(&cont.chars().collect::<Vec<_>>());
    let body_w = width.saturating_sub(first_w.max(cont_w)).max(1);
    let mut first_row = true;
    for src in text.split('\n') {
        let chars: Vec<char> = src.chars().collect();
        for (s, e) in wrap::wrap_ranges(&chars, body_w) {
            let prefix = if first_row { first } else { cont };
            first_row = false;
            let content: String = chars[s..e].iter().collect();
            lines.push(Line::from(vec![
                Span::styled(prefix.to_string(), style),
                Span::styled(content, style),
            ]));
        }
    }
}

/// Ближайшая (вниз) валидная граница символа для байтового смещения.
fn clamp_boundary(text: &str, mut off: usize) -> usize {
    while off < text.len() && !text.is_char_boundary(off) {
        off += 1;
    }
    off.min(text.len())
}

/// Добавляет тело сообщения: markdown для user, dim-текст для заметок.
fn push_body(
    lines: &mut Vec<Line<'static>>,
    item: &FeedMessage,
    palette: &Palette,
    width: usize,
    opts: markdown::RenderOpts,
) {
    if item.text.is_empty() {
        if item.streaming {
            lines.push(Line::from("…").dim());
        }
        return;
    }
    match item.role {
        FeedRole::Note => {
            for line in item.text.split('\n') {
                lines.push(Line::from(Span::from(line.to_string()).dim()));
            }
        }
        _ => {
            // markdown → Text; переносим строки в общий буфер. Для сообщения
            // пользователя одиночные переводы строки (Shift+Enter) сохраняем как
            // реальные переносы (GFM-стиль), иначе текст слился бы в один абзац.
            let opts = markdown::RenderOpts {
                soft_break_as_newline: true,
                ..opts
            };
            let rendered = markdown::render_with(&item.text, width, palette, opts);
            lines.extend(rendered.lines);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn ru() -> &'static Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
    }

    fn msg(role: FeedRole, text: &str, thoughts: &str) -> FeedMessage {
        FeedMessage {
            role,
            text: text.to_string(),
            thoughts: thoughts.to_string(),
            tools: Vec::new(),
            streaming: false,
        }
    }

    #[test]
    fn tool_blocks_render() {
        let mut feed = MessageFeed::new();
        let mut m = msg(FeedRole::Assistant, "готово", "");
        m.tools.push(FeedToolCall {
            name: "note_save".into(),
            arguments: "{\"content\":\"x\"}".into(),
            result: "Заметка сохранена".into(),
            text_offset: m.text.len(),
        });
        let lines = feed.build_lines(&[m], &Palette::default(), 80, ru());
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("⚒") && joined.contains("note_save"));
        assert!(joined.contains("Заметка сохранена"));
    }

    #[test]
    fn role_headers_localized_for_all_langs() {
        // Заголовки ролей ленты (ось B) следуют языку интерфейса: под `en` — «YOU»/
        // «ASSISTANT», под `ru` — «ВЫ»/«АССИСТЕНТ» (без кириллицы в английском).
        for &lang in crate::shared::i18n::Lang::ALL {
            let loc = crate::shared::i18n::locale(lang);
            let mut feed = MessageFeed::new();
            let msgs = vec![
                msg(FeedRole::User, "hi", ""),
                msg(FeedRole::Assistant, "ok", ""),
            ];
            let lines = feed.build_lines(&msgs, &Palette::default(), 80, loc);
            let joined: String = lines
                .iter()
                .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
                .collect();
            assert!(
                joined.contains(loc.t("ui.feed.role.user"))
                    && joined.contains(loc.t("ui.feed.role.assistant")),
                "{lang:?}: заголовки ролей не из бандла"
            );
        }
        // Явно: en-заголовки латиницей.
        let en = crate::shared::i18n::locale(crate::shared::i18n::Lang::En);
        assert_eq!(en.t("ui.feed.role.assistant"), "ASSISTANT");
        assert_eq!(en.t("ui.feed.role.user"), "YOU");
    }

    #[test]
    fn compat_palette_renders_without_emoji() {
        // Режим совместимости: заголовки ролей, свёрнутые «мысли» и tool-карточка
        // рисуются безопасными глифами — эмодзи/редких символов в ленте нет.
        let mut feed = MessageFeed::new();
        let mut m = msg(FeedRole::Assistant, "готово", "думал");
        m.tools.push(FeedToolCall {
            name: "note_save".into(),
            arguments: "{}".into(),
            result: "ок".into(),
            text_offset: m.text.len(),
        });
        let user = msg(FeedRole::User, "привет", "");
        let compat = Palette::default().with_compat(true);
        let lines = feed.build_lines(&[user, m], &compat, 80, ru());
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("* АССИСТЕНТ") && joined.contains("> ВЫ"));
        assert!(
            joined.contains("# note_save"),
            "ASCII tool-префикс: {joined}"
        );
        assert!(joined.contains("► мысли"), "компат-пилюля мыслей: {joined}");
        for banned in ['✦', '❯', '⚒', '▸'] {
            assert!(!joined.contains(banned), "остался {banned}: {joined}");
        }
    }

    #[test]
    fn python_tool_renders_highlighted_code_and_console() {
        // python_exec: код аргумента — подсвеченным блоком (RGB-цвета), результат —
        // консольной секцией stdout с содержимым.
        let mut feed = MessageFeed::new();
        let mut m = msg(FeedRole::Assistant, "готово", "");
        m.tools.push(FeedToolCall {
            name: "python_exec".into(),
            arguments: r#"{"code":"print(42)"}"#.into(),
            result: "stdout:\n42".into(),
            text_offset: m.text.len(),
        });
        let palette = Palette::for_theme(crate::shared::config::Theme::Dark);
        let lines = feed.build_lines(&[m], &palette, 80, ru());
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        // Заголовок без сырого JSON, код и консоль присутствуют.
        assert!(joined.contains("python_exec"), "{joined}");
        assert!(
            !joined.contains("{\"code\""),
            "сырой JSON не должен показываться"
        );
        assert!(joined.contains("print(42)"), "код: {joined}");
        assert!(
            joined.contains("stdout") && joined.contains("42"),
            "консоль: {joined}"
        );
        // Подсветка проставила RGB-цвет хотя бы одному спану кода.
        let has_rgb = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .any(|s| s.content.contains("print") && matches!(s.style.fg, Some(Color::Rgb(..))));
        assert!(has_rgb, "ожидался подсвеченный (RGB) спан кода");
    }

    #[test]
    fn python_stderr_uses_error_color() {
        let mut feed = MessageFeed::new();
        let palette = Palette::default();
        let mut m = msg(FeedRole::Assistant, "", "");
        m.tools.push(FeedToolCall {
            name: "python_exec".into(),
            arguments: r#"{"code":"raise SystemExit(1)"}"#.into(),
            result: "stderr:\nTraceback here\n\nкод возврата: 1".into(),
            text_offset: 0,
        });
        let lines = feed.build_lines(&[m], &palette, 80, ru());
        // Строка с текстом stderr окрашена цветом ошибки.
        let err_line = lines
            .iter()
            .find(|l| l.spans.iter().any(|s| s.content.contains("Traceback here")));
        let err_line = err_line.expect("строка stderr");
        assert!(
            err_line
                .spans
                .iter()
                .any(|s| s.content.contains("Traceback") && s.style.fg == Some(palette.error)),
            "stderr должен быть цветом ошибки"
        );
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("код возврата: 1"), "{joined}");
    }

    #[test]
    fn tool_call_renders_after_preceding_text_inline() {
        // Текст до вызова → tool-блок → текст после вызова: проверяем порядок строк.
        let mut feed = MessageFeed::new();
        let mut m = msg(FeedRole::Assistant, "Ищу погоду.\n\nГотово: ясно.", "");
        let off = "Ищу погоду.".len();
        m.tools.push(FeedToolCall {
            name: "web_search".into(),
            arguments: "{}".into(),
            result: "ясно".into(),
            text_offset: off,
        });
        let lines = feed.build_lines(&[m], &Palette::default(), 80, ru());
        let rows: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        let idx_before = rows.iter().position(|r| r.contains("Ищу погоду")).unwrap();
        let idx_tool = rows.iter().position(|r| r.contains("web_search")).unwrap();
        let idx_after = rows.iter().position(|r| r.contains("Готово")).unwrap();
        assert!(
            idx_before < idx_tool && idx_tool < idx_after,
            "ожидался порядок: текст-до < вызов < текст-после, было {idx_before}/{idx_tool}/{idx_after}"
        );
    }

    /// Хелпер: рендер строк в список «есть ли в строке непустой контент» —
    /// удобно искать пустые строки-разделители.
    fn row_texts(lines: &[Line<'static>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    /// Пустой ли визуальный ряд после снятия гуттер-рейла (`▌` + пробелы).
    fn is_blank_row(r: &str) -> bool {
        r.trim_matches(|c| c == '▌' || c == ' ').is_empty()
    }

    #[test]
    fn tool_block_separated_from_text_by_blank_lines() {
        // текст-до → вызов → текст-после: вокруг вызова должны быть пустые строки.
        let mut feed = MessageFeed::new();
        let mut m = msg(FeedRole::Assistant, "доПОСЛЕ", "");
        m.tools.push(FeedToolCall {
            name: "note_save".into(),
            arguments: String::new(),
            result: String::new(),
            text_offset: "до".len(),
        });
        let rows = row_texts(&feed.build_lines(&[m], &Palette::default(), 80, ru()));
        let i_before = rows.iter().position(|r| r.contains("до")).unwrap();
        let i_tool = rows.iter().position(|r| r.contains("note_save")).unwrap();
        let i_after = rows.iter().position(|r| r.contains("ПОСЛЕ")).unwrap();
        // Между текстом-до и вызовом — ровно одна пустая строка (с рейлом-гуттером).
        assert!(rows[i_before + 1..i_tool].iter().all(|r| is_blank_row(r)));
        assert_eq!(
            i_tool - i_before,
            2,
            "ожидалась одна пустая строка перед вызовом"
        );
        // Между вызовом и текстом-после — ровно одна пустая строка.
        assert_eq!(
            i_after - i_tool,
            2,
            "ожидалась одна пустая строка после вызова"
        );
    }

    #[test]
    fn tool_last_in_message_keeps_railed_trailing_blank() {
        // Вызов — последний элемент сообщения (результат = финал хода, типично для
        // python_exec). После карточки должен идти РЕЙЛОВЫЙ отступ (рейл продолжается
        // под результатом), а не безрейловый межсообщенческий разделитель, и ровно
        // один (без двойного пропуска).
        let mut feed = MessageFeed::new();
        let mut m = msg(FeedRole::Assistant, "Считаю.", "");
        m.tools.push(FeedToolCall {
            name: "python_exec".into(),
            arguments: r#"{"code":"print(1)"}"#.into(),
            result: "stdout:\n1".into(),
            text_offset: "Считаю.".len(),
        });
        let next = msg(FeedRole::User, "дальше", "");
        let rows = row_texts(&feed.build_lines(&[m, next], &Palette::default(), 60, ru()));
        let i_result = rows.iter().position(|r| r.contains('1')).unwrap();
        let i_next = rows.iter().position(|r| r.contains("дальше")).unwrap();
        // Между результатом и заголовком следующего сообщения — ровно одна пустая
        // строка, и она с рейлом (не безрейловый разделитель `[]`).
        let between: Vec<&String> = rows[i_result + 1..i_next].iter().collect();
        let blanks = between.iter().filter(|r| is_blank_row(r)).count();
        assert_eq!(blanks, 1, "ожидалась одна пустая строка: {between:?}");
        assert!(
            rows[i_next - 1].contains('▌'),
            "отступ после карточки должен нести рейл: {:?}",
            rows[i_next - 1]
        );
    }

    #[test]
    fn consecutive_tool_blocks_have_single_blank_between() {
        let mut feed = MessageFeed::new();
        let mut m = msg(FeedRole::Assistant, "", "");
        for name in ["first_tool", "second_tool"] {
            m.tools.push(FeedToolCall {
                name: name.into(),
                arguments: String::new(),
                result: String::new(),
                text_offset: 0,
            });
        }
        let rows = row_texts(&feed.build_lines(&[m], &Palette::default(), 80, ru()));
        let i1 = rows.iter().position(|r| r.contains("first_tool")).unwrap();
        let i2 = rows.iter().position(|r| r.contains("second_tool")).unwrap();
        // Между двумя подряд идущими вызовами — ровно одна пустая строка.
        assert_eq!(
            i2 - i1,
            2,
            "между соседними вызовами должна быть одна пустая строка"
        );
        assert!(rows[i1 + 1..i2].iter().all(|r| is_blank_row(r)));
    }

    #[test]
    fn long_tool_result_wraps_not_truncated() {
        let mut feed = MessageFeed::new();
        let mut m = msg(FeedRole::Assistant, "ок", "");
        let long = "слово ".repeat(40); // ~240 символов — заведомо шире узкой ленты
        m.tools.push(FeedToolCall {
            name: "web_search".into(),
            arguments: String::new(),
            result: long.trim_end().into(),
            text_offset: 0,
        });
        let lines = feed.build_lines(&[m], &Palette::default(), 30, ru());
        // Результат не обрезан (нет «…») и разложен на несколько рядов (рейл + гуттер
        // `└`/отступ продолжения).
        let gutter_rows = lines
            .iter()
            .filter(|l| {
                let s: String = l.spans.iter().map(|sp| sp.content.as_ref()).collect();
                s.contains("слово")
            })
            .count();
        assert!(
            gutter_rows > 1,
            "длинный результат должен переноситься, рядов: {gutter_rows}"
        );
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(
            !joined.contains('…'),
            "результат не должен обрезаться многоточием"
        );
    }

    #[test]
    fn followup_starts_new_bubble_and_hides_control_tool() {
        use crate::entities::message::{Message, MessageRole, ToolCallRecord};

        // A1 (с вызовом send_followup_message) → tool → A2 (new_bubble).
        let mut a1 = Message::assistant("Первое сообщение.");
        a1.tool_calls = vec![ToolCallRecord {
            thought_signature: None,
            id: "c1".into(),
            name: "send_followup_message".into(),
            arguments: serde_json::json!({}),
            result: Some("ок".into()),
        }];
        let tool_msg = {
            let mut m = Message::new(MessageRole::Tool, "ок");
            m.tool_call_id = Some("c1".into());
            m.tool_name = Some("send_followup_message".into());
            m
        };
        let mut a2 = Message::assistant("Второе сообщение.");
        a2.new_bubble = true;

        let feed = FeedMessage::from_messages(&[a1, tool_msg, a2]);
        // Два отдельных пузыря ассистента (не склеены).
        assert_eq!(feed.len(), 2);
        assert_eq!(feed[0].text, "Первое сообщение.");
        assert_eq!(feed[1].text, "Второе сообщение.");
        // Служебный вызов управляющего инструмента в ленте не показывается.
        assert!(
            feed[0].tools.is_empty(),
            "control-вызов не должен давать tool-блок"
        );
    }

    #[test]
    fn assistant_rounds_without_new_bubble_still_merge() {
        use crate::entities::message::{Message, MessageRole, ToolCallRecord};
        // Обычный agentic-раунд (note_save) по-прежнему склеивается в один пузырь.
        let mut a1 = Message::assistant("Ищу.");
        a1.tool_calls = vec![ToolCallRecord {
            thought_signature: None,
            id: "c1".into(),
            name: "note_save".into(),
            arguments: serde_json::json!({}),
            result: Some("ok".into()),
        }];
        let tool_msg = {
            let mut m = Message::new(MessageRole::Tool, "ok");
            m.tool_call_id = Some("c1".into());
            m
        };
        let a2 = Message::assistant("Готово.");
        let feed = FeedMessage::from_messages(&[a1, tool_msg, a2]);
        assert_eq!(feed.len(), 1, "обычные раунды склеиваются");
        assert_eq!(feed[0].tools.len(), 1, "обычный tool-блок виден");
    }

    #[test]
    fn from_message_skips_system() {
        let sys = Message::new(MessageRole::System, "s");
        assert!(FeedMessage::from_message(&sys).is_none());
        let user = Message::user("hi");
        assert_eq!(
            FeedMessage::from_message(&user).unwrap().role,
            FeedRole::User
        );
    }

    #[test]
    fn collapsed_thoughts_show_indicator_not_content() {
        let mut feed = MessageFeed::new(); // show_thoughts = false
        let lines = feed.build_lines(
            &[msg(FeedRole::Assistant, "ответ", "секрет\nмысль")],
            &Palette::default(),
            80,
            ru(),
        );
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("▸ мысли"));
        assert!(!joined.contains("секрет"));
    }

    #[test]
    fn expanded_thoughts_show_content() {
        let mut feed = MessageFeed::new();
        feed.toggle_thoughts();
        let lines = feed.build_lines(
            &[msg(FeedRole::Assistant, "ответ", "секрет")],
            &Palette::default(),
            80,
            ru(),
        );
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("секрет"));
    }

    #[test]
    fn user_message_preserves_single_newlines() {
        // Сообщение пользователя с Shift+Enter (одиночный \n) должно сохранять
        // переносы строк, а не сливаться в один абзац (GFM-стиль).
        let mut feed = MessageFeed::new();
        let lines = feed.build_lines(
            &[msg(FeedRole::User, "Привет!\nКак дела?", "")],
            &Palette::default(),
            80,
            ru(),
        );
        let rows = row_texts(&lines);
        let i_first = rows.iter().position(|r| r.contains("Привет!")).unwrap();
        let i_second = rows.iter().position(|r| r.contains("Как дела?")).unwrap();
        assert_ne!(
            i_first, i_second,
            "две строки должны оказаться на разных визуальных рядах"
        );
        // И ни одна строка не должна содержать обе фразы (не склеены в одну).
        assert!(
            !rows
                .iter()
                .any(|r| r.contains("Привет!") && r.contains("Как дела?")),
            "строки не должны сливаться в один ряд"
        );
    }

    #[test]
    fn markdown_is_applied_to_body() {
        let mut feed = MessageFeed::new();
        // LaTeX действует внутри $…$ (delimiter-scoped, см. shared::markdown)
        let lines = feed.build_lines(
            &[msg(FeedRole::Assistant, "формула $x^2$", "")],
            &Palette::default(),
            80,
            ru(),
        );
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("x²"));
    }

    #[test]
    fn rail_is_not_dimmed_next_to_table_borders() {
        // Рейл слева должен иметь чистый цвет роли без line-level модификаторов
        // (напр. DIM у рамок таблиц/разделителей), иначе он другого цвета напротив них.
        let mut feed = MessageFeed::new();
        let table = "| a | b |\n|---|---|\n| 1 | 2 |";
        let lines = feed.build_lines(
            &[msg(FeedRole::Assistant, table, "")],
            &Palette::default(),
            80,
            ru(),
        );
        for line in &lines {
            // Рейл — первый спан с символом «▌».
            if let Some(rail) = line.spans.first()
                && rail.content.starts_with('▌')
            {
                assert!(
                    !rail.style.add_modifier.contains(Modifier::DIM),
                    "рейл не должен наследовать DIM от строки таблицы/разделителя"
                );
            }
        }
    }

    #[test]
    fn table_row_separators_follow_setting_and_invalidate_cache() {
        // По умолчанию (зеркало дефолта конфига) разделителей между строками нет —
        // только под заголовком; включение настройки добавляет их, а смена значения
        // сбрасывает кэш (сообщение/ширина/палитра те же — меняется только флаг).
        let mut feed = MessageFeed::new();
        let palette = Palette::default();
        let table = "| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |";
        let mids = |lines: &[Line<'static>]| {
            lines
                .iter()
                .filter(|l| l.spans.iter().any(|s| s.content.contains('├')))
                .count()
        };
        let off = feed.build_lines(&[msg(FeedRole::Assistant, table, "")], &palette, 80, ru());
        assert_eq!(mids(&off), 1, "по умолчанию: только под заголовком");
        feed.set_table_row_separators(true);
        let on = feed.build_lines(&[msg(FeedRole::Assistant, table, "")], &palette, 80, ru());
        assert_eq!(
            mids(&on),
            2,
            "включено: разделитель заголовка + один межстрочный (кэш сброшен по ключу)"
        );
    }

    #[test]
    fn render_mermaid_follows_setting_and_invalidates_cache() {
        // По умолчанию (зеркало дефолта конфига) mermaid-блок рендерится диаграммой;
        // выключение настройки возвращает исходник, а смена значения сбрасывает кэш
        // (сообщение/ширина/палитра те же — меняется только флаг).
        let mut feed = MessageFeed::new();
        let palette = Palette::default();
        let md = "```mermaid\nflowchart LR\n    A[Start] --> B[End]\n```";
        let joined = |lines: &[Line<'static>]| -> String {
            lines
                .iter()
                .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
                .collect()
        };
        let on = feed.build_lines(&[msg(FeedRole::Assistant, md, "")], &palette, 90, ru());
        assert!(
            joined(&on).contains('┌') && !joined(&on).contains("```"),
            "по умолчанию включено — диаграмма, не исходник"
        );
        feed.set_render_mermaid(false);
        let off = feed.build_lines(&[msg(FeedRole::Assistant, md, "")], &palette, 90, ru());
        assert!(
            joined(&off).contains("```mermaid"),
            "выключено — исходник (кэш сброшен по ключу)"
        );
    }

    #[test]
    fn streamed_mermaid_stays_source_until_fence_closes() {
        // Симуляция стрима: пока закрывающий забор не доехал, блок показывается
        // исходником (не мерцающим огрызком диаграммы); с приходом забора
        // фингерпринт сообщения меняется, кэш пересчитывает блок → диаграмма.
        let mut feed = MessageFeed::new();
        let palette = Palette::default();
        let joined = |lines: &[Line<'static>]| -> String {
            lines
                .iter()
                .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
                .collect()
        };
        // Чанк 1: незакрытый, но синтаксически валидный огрызок.
        let partial = "```mermaid\nflowchart LR\n    A[Start] --> B[End]";
        let mid = feed.build_lines(&[msg(FeedRole::Assistant, partial, "")], &palette, 90, ru());
        assert!(
            joined(&mid).contains("```mermaid") && !joined(&mid).contains('┌'),
            "во время стрима — исходник, не диаграмма: {}",
            joined(&mid)
        );
        // Чанк 2: доехал закрывающий забор — то же сообщение, текст дописан.
        let full = "```mermaid\nflowchart LR\n    A[Start] --> B[End]\n```";
        let done = feed.build_lines(&[msg(FeedRole::Assistant, full, "")], &palette, 90, ru());
        assert!(
            joined(&done).contains('┌') && !joined(&done).contains("```"),
            "по дописывании забора — диаграмма: {}",
            joined(&done)
        );
    }

    #[test]
    fn scroll_up_disables_follow() {
        let mut feed = MessageFeed::new();
        assert!(feed.follow);
        feed.scroll_up(3);
        assert!(!feed.follow);
    }

    #[test]
    fn scroll_sets_take_once_flag() {
        // Прокрутка (вверх/вниз) взводит флаг, `take_scrolled` забирает его один раз
        // (петля по нему делает полную перерисовку — стирает артефакты VS16-эмодзи).
        let mut feed = MessageFeed::new();
        assert!(!feed.take_scrolled(), "до прокрутки флаг не взведён");
        feed.scroll_up(3);
        assert!(feed.take_scrolled(), "после прокрутки флаг взведён");
        assert!(!feed.take_scrolled(), "флаг забирается однократно");
        feed.scroll_down(3);
        assert!(feed.take_scrolled());
    }

    #[test]
    fn render_does_not_panic() {
        let mut feed = MessageFeed::new();
        let messages = vec![
            msg(FeedRole::User, "привет", ""),
            msg(
                FeedRole::Assistant,
                "# Заголовок\n\nответ с `кодом`",
                "мысль",
            ),
            FeedMessage::note("(генерация отменена)"),
        ];
        let mut term = Terminal::new(TestBackend::new(40, 10)).unwrap();
        term.draw(|f| {
            feed.render(
                f,
                f.area(),
                "Чат",
                "gemma-4 · 16k ctx",
                &messages,
                &Palette::default(),
                ru(),
            )
        })
        .unwrap();
    }

    #[test]
    fn scrollbar_appears_only_when_feed_overflows() {
        // Бегунок «█» на правой рамке — только когда строк больше высоты ленты.
        let right_col = |term: &Terminal<TestBackend>| -> Vec<String> {
            let buf = term.backend().buffer();
            let area = buf.area;
            (area.top()..area.bottom())
                .map(|y| buf[(area.right() - 1, y)].symbol().to_string())
                .collect()
        };
        let mut feed = MessageFeed::new();
        let mut term = Terminal::new(TestBackend::new(30, 8)).unwrap();
        let short = vec![msg(FeedRole::User, "привет", "")];
        term.draw(|f| feed.render(f, f.area(), "Чат", "", &short, &Palette::default(), ru()))
            .unwrap();
        assert!(
            !right_col(&term).iter().any(|s| s == "█"),
            "короткая лента — без бегунка"
        );
        let many: Vec<FeedMessage> = (0..30)
            .map(|i| msg(FeedRole::User, &format!("строка {i}"), ""))
            .collect();
        term.draw(|f| feed.render(f, f.area(), "Чат", "", &many, &Palette::default(), ru()))
            .unwrap();
        assert!(
            right_col(&term).iter().any(|s| s == "█"),
            "переполненная лента — с бегунком"
        );
    }

    /// Собирает содержимое всех спанов строки в кортеж (текст, fg) — для сравнения.
    fn line_sig(l: &Line<'static>) -> Vec<(String, Option<Color>)> {
        l.spans
            .iter()
            .map(|s| (s.content.to_string(), s.style.fg))
            .collect()
    }

    /// Тёплый кэш даёт построчно идентичный вывод свежему рендеру — по разным
    /// сценариям, ширинам, палитрам и состоянию показа мыслей.
    #[test]
    fn cache_matches_fresh_render() {
        let tool_msg = {
            let mut m = msg(FeedRole::Assistant, "готово", "");
            m.tools.push(FeedToolCall {
                name: "note_save".into(),
                arguments: "{}".into(),
                result: "ок".into(),
                text_offset: m.text.len(),
            });
            m
        };
        let scenarios: Vec<Vec<FeedMessage>> = vec![
            vec![msg(FeedRole::User, "привет как дела сегодня", "")],
            vec![
                msg(FeedRole::User, "вопрос", ""),
                msg(
                    FeedRole::Assistant,
                    "# Заголовок\n\nответ с `кодом` и формулой $x^2 + \\alpha$",
                    "рассуждение модели",
                ),
            ],
            vec![tool_msg],
        ];
        for messages in &scenarios {
            for width in [40usize, 80] {
                for palette in [Palette::default(), Palette::default().with_compat(true)] {
                    for show in [false, true] {
                        let mut warm = MessageFeed::new();
                        if show {
                            warm.toggle_thoughts();
                        }
                        // прогреваем кэш повторными вызовами
                        let _ = warm.build_lines(messages, &palette, width, ru());
                        let _ = warm.build_lines(messages, &palette, width, ru());
                        let warm_lines = warm.build_lines(messages, &palette, width, ru());

                        let mut fresh = MessageFeed::new();
                        if show {
                            fresh.toggle_thoughts();
                        }
                        let fresh_lines = fresh.build_lines(messages, &palette, width, ru());

                        let w: Vec<_> = warm_lines.iter().map(line_sig).collect();
                        let f: Vec<_> = fresh_lines.iter().map(line_sig).collect();
                        assert_eq!(w, f, "кэш разошёлся: width={width} show={show}");
                    }
                }
            }
        }
    }

    /// Стриминг: на каждом шаге роста текста тёплый кэш совпадает со свежим рендером
    /// (пересчитывается только хвостовое сообщение).
    #[test]
    fn cache_matches_fresh_during_streaming() {
        let mut warm = MessageFeed::new();
        let palette = Palette::default();
        let full = "Это ответ ассистента, который растёт по чанкам стриминга.";
        let total = full.chars().count();
        for end in (1..=total).step_by(3) {
            let partial: String = full.chars().take(end).collect();
            let mut m = msg(FeedRole::Assistant, &partial, "");
            m.streaming = true;
            let messages = vec![msg(FeedRole::User, "спроси", ""), m];
            let warm_lines = warm.build_lines(&messages, &palette, 60, ru());
            let mut fresh = MessageFeed::new();
            let fresh_lines = fresh.build_lines(&messages, &palette, 60, ru());
            let w: Vec<_> = warm_lines.iter().map(line_sig).collect();
            let f: Vec<_> = fresh_lines.iter().map(line_sig).collect();
            assert_eq!(w, f, "стрим-кэш разошёлся на end={end}");
        }
    }

    /// Правка текста сообщения инвалидирует кэш (новый текст виден, старого нет).
    #[test]
    fn cache_invalidates_on_message_edit() {
        let mut feed = MessageFeed::new();
        let palette = Palette::default();
        let join = |ls: &[Line<'static>]| -> String {
            ls.iter()
                .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
                .collect()
        };
        let l1 = feed.build_lines(
            &[msg(FeedRole::Assistant, "первый вариант", "")],
            &palette,
            80,
            ru(),
        );
        assert!(join(&l1).contains("первый вариант"));
        let l2 = feed.build_lines(
            &[msg(FeedRole::Assistant, "другой текст", "")],
            &palette,
            80,
            ru(),
        );
        let j2 = join(&l2);
        assert!(j2.contains("другой текст"), "{j2}");
        assert!(!j2.contains("первый вариант"), "устаревший кэш: {j2}");
    }

    /// Усечение истории (Ctrl+E/regenerate): после укорачивания вывод тёплого кэша
    /// совпадает со свежим (хвост кэша отброшен).
    #[test]
    fn cache_handles_history_truncation() {
        let mut feed = MessageFeed::new();
        let palette = Palette::default();
        let long = vec![
            msg(FeedRole::User, "раз", ""),
            msg(FeedRole::Assistant, "ответ раз", ""),
            msg(FeedRole::User, "два", ""),
            msg(FeedRole::Assistant, "ответ два", ""),
        ];
        let _ = feed.build_lines(&long, &palette, 80, ru());
        let warm = feed.build_lines(&long[..2], &palette, 80, ru());
        let mut fresh = MessageFeed::new();
        let fresh_lines = fresh.build_lines(&long[..2], &palette, 80, ru());
        let w: Vec<_> = warm.iter().map(line_sig).collect();
        let f: Vec<_> = fresh_lines.iter().map(line_sig).collect();
        assert_eq!(w, f);
    }
}
