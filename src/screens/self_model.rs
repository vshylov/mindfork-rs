//! Экран «модели себя» (FSD "page"): просмотр и **ручная правка** представления
//! агента о себе для активного профиля — описание, цели, модель собеседника и
//! нарратив инсайтов. Открывается из чата по `F3`, закрывается `Esc`.
//!
//! Данные приходят снимком от оркестратора (он владеет `Storage`) событием
//! `AppEvent::SelfModelView`. Правки экран отдаёт намерением `SelfModelIntent::Edit`
//! (→ `AppCommand::UpdateSelfModel`); оркестратор применяет, сохраняет и переэмитит
//! обновлённый снимок (экран обновляется на месте). Сам экран про `app`/`Storage`
//! не знает (FSD). См. docs/history/self-model-mvp.md.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use uuid::Uuid;

use crate::entities::self_model::{GoalStatus, SelfModel, SelfModelEdit};
use crate::shared::keys;
use crate::shared::theme::Palette;
use crate::shared::ui::dim_background;
use crate::shared::wrap::wrap_line;
use crate::widgets::input_box::InputBox;

/// Намерение экрана «модели себя» (транслируется `app`). Параллель к
/// [`ChatListIntent`](crate::screens::chat_list::ChatListIntent).
#[derive(Debug, Clone, PartialEq)]
pub enum SelfModelIntent {
    /// Закрыть вид, вернуться к чату (`Esc`).
    Close,
    /// Выйти из приложения (`Ctrl+C`).
    Quit,
    /// Применить ручную правку (оркестратор сохранит и переэмитит снимок).
    Edit(SelfModelEdit),
}

/// На что указывает выбранная строка (для действий правки).
#[derive(Debug, Clone, Copy, PartialEq)]
enum RowAction {
    Summary,
    Goal(Uuid),
    AddGoal,
    Traits,
    Interests,
    Relationship,
    Insight(Uuid),
    /// Декоративная строка (пустой разделитель или заголовок секции) — на ней
    /// нельзя стоять курсором; навигация её пропускает.
    Decoration,
}

/// Что именно редактируется в открытом текстовом редакторе.
#[derive(Debug, Clone, Copy, PartialEq)]
enum EditKind {
    Summary,
    AddGoal,
    GoalText(Uuid),
    Traits,
    Interests,
    Relationship,
}

/// Активный текстовый редактор поля (попап). Везде многострочный
/// (`Shift+Enter` — перенос строки, `Enter` — коммит): модели пишут длинный текст.
struct Editor {
    kind: EditKind,
    input: InputBox,
}

/// Экран «модели себя»: снимок модели + состояние навигации/правки.
pub struct SelfModelScreen {
    model: Option<SelfModel>,
    palette: Palette,
    selected: usize,
    /// Первый видимый визуальный ряд (для прокрутки длинного списка). Держится
    /// между кадрами; пересчитывается в [`Self::render`] так, чтобы выбранная строка
    /// оставалась видимой (а если не влезает целиком — был виден её верх).
    scroll: usize,
    editor: Option<Editor>,
    /// Подтверждение очистки всей модели (`Ctrl+K` дважды).
    confirm_clear: bool,
}

impl SelfModelScreen {
    /// Открывает экран со снимком модели.
    pub fn new(model: Option<SelfModel>, palette: Palette) -> Self {
        Self {
            model,
            palette,
            selected: 0,
            scroll: 0,
            editor: None,
            confirm_clear: false,
        }
    }

    /// Обновляет снимок модели (после правки/рефлексии — событие `SelfModelView`),
    /// сохраняя выделение по возможности и закрывая подтверждение очистки.
    pub fn set_model(&mut self, model: Option<SelfModel>) {
        self.model = model;
        self.confirm_clear = false;
        let max = self.rows().len().saturating_sub(1);
        self.selected = self.selected.min(max);
        // Снимок мог сдвинуть/убрать строки — увести курсор с декорации, если попал.
        self.move_selection(0);
    }

    /// Обновляет палитру темы (событие `AppEvent::Settings`).
    pub fn set_palette(&mut self, palette: Palette) {
        self.palette = palette;
    }

    /// Строит навигируемые строки (вид + цель действия). Базовые поля присутствуют
    /// всегда (правка возможна и на пустой модели); цели и инсайты — по наличию.
    fn rows(&self) -> Vec<(Line<'static>, RowAction)> {
        let p = &self.palette;
        let empty = SelfModel::new(Uuid::nil());
        let m = self.model.as_ref().unwrap_or(&empty);
        let label = |s: &str| Span::styled(s.to_string(), p.accent_style());
        let dim = |s: String| Span::styled(s, p.muted_style());
        let mut rows: Vec<(Line<'static>, RowAction)> = Vec::new();
        // Декоративные строки-разделители: пустая строка и заголовок секции
        // (жирным акцентом). На них курсор не встаёт — лишь визуально делят секции.
        let spacer = || (Line::from(String::new()), RowAction::Decoration);
        let header = |s: &str| {
            (
                Line::from(Span::styled(
                    s.to_string(),
                    p.accent_style().add_modifier(Modifier::BOLD),
                )),
                RowAction::Decoration,
            )
        };

        // Описание себя.
        let summary = if m.summary.trim().is_empty() {
            dim("—".into())
        } else {
            Span::raw(m.summary.trim().to_string())
        };
        rows.push((
            Line::from(vec![label("О себе: "), summary]),
            RowAction::Summary,
        ));

        // Цели (с маркером статуса) + строка добавления. Дата — в локальной зоне:
        // для активной — создание, для закрытой — момент закрытия (`closed_at`).
        for g in &m.goals {
            let (marker, style) = match g.status {
                GoalStatus::Active => ("● ", p.success_style()),
                GoalStatus::Completed => ("✓ ", p.muted_style()),
                GoalStatus::Abandoned => ("✗ ", p.muted_style()),
            };
            let date = g
                .closed_at
                .unwrap_or(g.created_at)
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%d")
                .to_string();
            rows.push((
                Line::from(vec![
                    Span::styled(marker.to_string(), style),
                    Span::raw(g.description.trim().to_string()),
                    dim(format!("  [{date}]")),
                ]),
                RowAction::Goal(g.id),
            ));
        }
        rows.push((
            Line::from(vec![Span::styled(
                "＋ добавить цель".to_string(),
                p.muted_style(),
            )]),
            RowAction::AddGoal,
        ));

        // Модель собеседника — отделена пустой строкой и заголовком от секции «о себе».
        rows.push(spacer());
        rows.push(header("Собеседник"));
        let u = &m.user_model;
        let join_or_dash = |v: &[String]| {
            if v.is_empty() {
                dim("—".into())
            } else {
                Span::raw(v.join(", "))
            }
        };
        rows.push((
            Line::from(vec![label("Черты: "), join_or_dash(&u.perceived_traits)]),
            RowAction::Traits,
        ));
        rows.push((
            Line::from(vec![
                label("Интересы: "),
                join_or_dash(&u.current_interests),
            ]),
            RowAction::Interests,
        ));
        let rel = if u.relationship_dynamic.trim().is_empty() {
            dim("—".into())
        } else {
            Span::raw(u.relationship_dynamic.trim().to_string())
        };
        rows.push((
            Line::from(vec![label("Отношения: "), rel]),
            RowAction::Relationship,
        ));

        // Нарратив (новые сверху) — удаление по `Del`. Тоже за разделителем и
        // заголовком, чтобы наблюдения не сливались с моделью собеседника.
        if !m.narrative.is_empty() {
            rows.push(spacer());
            rows.push(header(&format!("Наблюдения ({})", m.narrative.len())));
        }
        for seg in m.narrative.iter().rev() {
            // Дата — в локальной зоне: `created_at` хранится в UTC, и без перевода
            // наблюдение, добавленное после локальной полуночи, показывало бы
            // вчерашний день (в зонах впереди UTC ещё идут «вчерашние» сутки).
            let date = seg
                .created_at
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%d")
                .to_string();
            rows.push((
                Line::from(vec![
                    dim(format!("[{date}] ")),
                    Span::raw(seg.text.trim().to_string()),
                ]),
                RowAction::Insight(seg.id),
            ));
        }
        rows
    }

    /// Действие выбранной строки (или `None`, если индекс вне диапазона).
    fn selected_action(&self) -> Option<RowAction> {
        self.rows().get(self.selected).map(|(_, a)| *a)
    }

    /// Индексы строк, на которых может стоять курсор (всё, кроме декораций).
    fn selectable_indices(&self) -> Vec<usize> {
        self.rows()
            .iter()
            .enumerate()
            .filter(|(_, (_, a))| !matches!(a, RowAction::Decoration))
            .map(|(i, _)| i)
            .collect()
    }

    /// Сдвигает выделение на `delta` позиций среди выбираемых строк (декорации
    /// пропускаются). `isize::MIN`/`MAX` — в начало/конец. Если текущая строка —
    /// декорация (после смены модели), стартуем от ближайшей выбираемой.
    fn move_selection(&mut self, delta: isize) {
        let sel = self.selectable_indices();
        if sel.is_empty() {
            return;
        }
        let cur = sel
            .iter()
            .position(|&i| i == self.selected)
            .unwrap_or_else(|| sel.iter().filter(|&&i| i < self.selected).count());
        let new = (cur as isize)
            .saturating_add(delta)
            .clamp(0, sel.len() as isize - 1) as usize;
        self.selected = sel[new];
    }

    /// Обрабатывает нажатие клавиши, возвращая намерение для `app`.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<SelfModelIntent> {
        if self.editor.is_some() {
            return self.handle_editor_key(key);
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let phys = if let KeyCode::Char(c) = key.code {
            keys::physical_char(c)
        } else {
            '\0'
        };
        // Подтверждение очистки: повторный Ctrl+K — да; любая другая клавиша — отмена.
        if self.confirm_clear {
            self.confirm_clear = false;
            if ctrl && phys == 'k' {
                return Some(SelfModelIntent::Edit(SelfModelEdit::Clear));
            }
            return None;
        }
        if ctrl && phys == 'c' {
            return Some(SelfModelIntent::Quit);
        }
        if ctrl && phys == 'k' {
            self.confirm_clear = true;
            return None;
        }
        match key.code {
            KeyCode::Esc => return Some(SelfModelIntent::Close),
            KeyCode::Up => self.move_selection(-1),
            KeyCode::Down => self.move_selection(1),
            KeyCode::PageUp => self.move_selection(-10),
            KeyCode::PageDown => self.move_selection(10),
            KeyCode::Home => self.move_selection(isize::MIN),
            KeyCode::End => self.move_selection(isize::MAX),
            KeyCode::Enter => return self.begin_edit(),
            KeyCode::Char(' ') => {
                if let Some(RowAction::Goal(id)) = self.selected_action() {
                    return Some(SelfModelIntent::Edit(SelfModelEdit::CycleGoalStatus(id)));
                }
            }
            KeyCode::Delete => match self.selected_action() {
                Some(RowAction::Goal(id)) => {
                    return Some(SelfModelIntent::Edit(SelfModelEdit::DeleteGoal(id)));
                }
                Some(RowAction::Insight(id)) => {
                    return Some(SelfModelIntent::Edit(SelfModelEdit::DeleteInsight(id)));
                }
                _ => {}
            },
            _ => {}
        }
        None
    }

    /// Открывает редактор для выбранной строки (или ничего — для инсайта).
    fn begin_edit(&mut self) -> Option<SelfModelIntent> {
        let action = self.selected_action()?;
        let m = self
            .model
            .clone()
            .unwrap_or_else(|| SelfModel::new(Uuid::nil()));
        let (kind, seed) = match action {
            RowAction::Summary => (EditKind::Summary, m.summary.clone()),
            RowAction::AddGoal => (EditKind::AddGoal, String::new()),
            RowAction::Goal(id) => {
                let seed = m
                    .goals
                    .iter()
                    .find(|g| g.id == id)
                    .map(|g| g.description.clone())
                    .unwrap_or_default();
                (EditKind::GoalText(id), seed)
            }
            RowAction::Traits => (EditKind::Traits, m.user_model.perceived_traits.join(", ")),
            RowAction::Interests => (
                EditKind::Interests,
                m.user_model.current_interests.join(", "),
            ),
            RowAction::Relationship => (
                EditKind::Relationship,
                m.user_model.relationship_dynamic.clone(),
            ),
            RowAction::Insight(_) => return None, // инсайты не правим, только удаляем
            RowAction::Decoration => return None, // разделитель/заголовок — не редактируется
        };
        // Поле многострочное (по умолчанию `InputBox` уже такой) — текст переносится.
        let mut input = InputBox::new();
        input.set_text(&seed);
        self.editor = Some(Editor { kind, input });
        None
    }

    fn handle_editor_key(&mut self, key: KeyEvent) -> Option<SelfModelIntent> {
        let editor = self.editor.as_mut()?;
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => {
                self.editor = None;
                None
            }
            (KeyCode::Enter, KeyModifiers::SHIFT) => {
                editor.input.insert_newline();
                None
            }
            (KeyCode::Enter, _) => {
                let editor = self.editor.take().unwrap();
                commit_edit(editor.kind, editor.input.text())
            }
            (KeyCode::Char(c), m)
                if m.contains(KeyModifiers::CONTROL) && keys::physical_char(c) == 'k' =>
            {
                editor.input.clear_or_restore();
                None
            }
            _ => {
                editor.input.on_key(key);
                None
            }
        }
    }

    /// Вставка из буфера обмена — в активный редактор поля (иначе no-op).
    pub fn handle_paste(&mut self, text: &str) {
        if let Some(editor) = self.editor.as_mut() {
            editor.input.insert_str(text);
        }
    }

    /// Рисует экран во весь экран: список полей + строка хоткеев; редактор — попапом.
    pub fn render(&mut self, frame: &mut Frame) {
        let area = frame.area();
        let palette = self.palette;
        let block = palette.panel("✦ Модель себя", true);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(inner);

        // Перенос по словам: длинные значения (модели пишут много текста) не влезают
        // в одну строку. Каждая логическая строка списка заворачивается на несколько
        // визуальных рядов; затем рисуем визуальные ряды вручную (а не виджетом
        // `List`). Причина: `List` целиком ПРОПУСКАЕТ многострочный элемент, который не
        // вмещается в остаток высоты по высоте (его `get_items_bounds` прерывается на
        // `height + item.height() > max_height`), оставляя пустоту — длинный пункт
        // внизу выглядит как «конец списка». Ручной рендер обрезает хвостовой пункт по
        // высоте области, показывая его верхнюю часть. Ширина содержимого = область
        // минус колонка маркера выделения `▌ ` (2 колонки).
        let list_area = chunks[0];
        let content_width = (list_area.width as usize).saturating_sub(2).max(1);
        let rows = self.rows();
        let sel = self.selected.min(rows.len().saturating_sub(1));

        // Разворачиваем каждую логическую строку в визуальные ряды, помня для каждого
        // ряда индекс его логической строки и где начинается выбранная строка.
        let mut visual: Vec<(usize, Line<'static>)> = Vec::new();
        let mut sel_start = 0usize;
        let mut sel_height = 1usize;
        for (ri, (line, _)) in rows.iter().enumerate() {
            let start = visual.len();
            if ri == sel {
                sel_start = start;
            }
            let mut wrapped = wrap_line(line, content_width);
            if wrapped.is_empty() {
                wrapped.push(Line::from(String::new())); // пустой разделитель
            }
            if ri == sel {
                sel_height = wrapped.len();
            }
            for vl in wrapped {
                visual.push((ri, vl));
            }
        }

        let view_h = list_area.height as usize;
        self.scroll = adjust_scroll(self.scroll, sel_start, sel_height, view_h);

        // Рисуем видимые визуальные ряды. У выбранной строки — подложка на всю ширину
        // ряда (база `Paragraph` красит всю область) и маркер `▌`, у прочих — отступ.
        for (offset, (ri, line)) in visual.iter().enumerate().skip(self.scroll).take(view_h) {
            let y = list_area.y + (offset - self.scroll) as u16;
            let row = Rect {
                x: list_area.x,
                y,
                width: list_area.width,
                height: 1,
            };
            let selected = *ri == sel;
            let prefix = if selected {
                Span::raw("▌ ")
            } else {
                Span::raw("  ")
            };
            let mut spans = vec![prefix];
            spans.extend(line.spans.iter().cloned());
            let mut para = Paragraph::new(Line::from(spans));
            if selected {
                para = para.style(Style::new().bg(palette.keycap_bg));
            }
            frame.render_widget(para, row);
        }

        // Нижняя строка: подтверждение очистки или хоткеи.
        if self.confirm_clear {
            let warn = ratatui::style::Style::new().fg(palette.warning);
            frame.render_widget(
                Paragraph::new(Line::styled(
                    "Очистить всю модель? Ctrl+K — да, любая клавиша — нет",
                    warn,
                )),
                chunks[1],
            );
        } else {
            let mut hint: Vec<Span<'static>> = Vec::new();
            for (k, d) in [
                ("Enter", "правка"),
                ("Space", "статус цели"),
                ("Del", "удалить"),
                ("Ctrl+K", "очистить"),
                ("Esc", "закрыть"),
            ] {
                hint.extend(palette.hint(k, d));
                hint.push(Span::raw("  "));
            }
            frame.render_widget(Paragraph::new(Line::from(hint)), chunks[1]);
        }

        // Редактор поверх — с реальным курсором. Везде многострочный (перенос текста).
        if let Some(editor) = self.editor.as_mut() {
            let popup = centered_rect(80, 50, area);
            let title = "правка · Shift+Enter перенос · Enter ок · Esc отмена";
            dim_background(frame);
            frame.render_widget(Clear, popup);
            editor
                .input
                .render(frame, popup, title, true, &palette, false);
        }
    }
}

/// Формирует намерение правки из коммита редактора (или `None`, если правка пуста).
fn commit_edit(kind: EditKind, text: String) -> Option<SelfModelIntent> {
    let edit = match kind {
        EditKind::Summary => SelfModelEdit::SetSummary(text),
        EditKind::AddGoal => {
            if text.trim().is_empty() {
                return None;
            }
            SelfModelEdit::AddGoal(text)
        }
        EditKind::GoalText(id) => SelfModelEdit::SetGoalText { id, text },
        EditKind::Traits => SelfModelEdit::SetTraits(parse_list(&text)),
        EditKind::Interests => SelfModelEdit::SetInterests(parse_list(&text)),
        EditKind::Relationship => SelfModelEdit::SetRelationship(text),
    };
    Some(SelfModelIntent::Edit(edit))
}

/// Список → вектор непустых обрезанных элементов. Разделители — запятая **и**
/// перевод строки (поле многострочное: элементы можно вводить как через запятую,
/// так и по одному на строку).
fn parse_list(s: &str) -> Vec<String> {
    s.split([',', '\n'])
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .collect()
}

/// Пересчитывает прокрутку так, чтобы выбранная строка (визуальные ряды
/// `[sel_start, sel_start + sel_height)`) была видна в окне высотой `view_h`:
/// - если строка выше окна — поднимаем верх окна к её началу;
/// - если её низ за окном — опускаем окно к её низу;
/// - но если строка целиком не вмещается (выше окна) — прижимаем к её **верху**
///   (видна верхняя часть длинного пункта, а не низ).
///
/// Иначе прокрутка не меняется.
fn adjust_scroll(scroll: usize, sel_start: usize, sel_height: usize, view_h: usize) -> usize {
    if view_h == 0 {
        return scroll;
    }
    let sel_end = sel_start + sel_height; // exclusive
    if sel_start < scroll {
        sel_start
    } else if sel_end > scroll + view_h {
        if sel_height >= view_h {
            sel_start
        } else {
            sel_end - view_h
        }
    } else {
        scroll
    }
}

/// Центрированный прямоугольник в процентах ширины/высоты.
fn centered_rect(pct_x: u16, pct_y: u16, area: Rect) -> Rect {
    let w = area.width * pct_x / 100;
    let h = area.height * pct_y / 100;
    Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// Добавляет наблюдение в снимок для тестов. Наблюдения переехали в заметки, а
    /// снимок `F3` несёт их реконструированными в поле `narrative` (оркестратор
    /// заполняет из self-заметок), поэтому в тестах кладём прямо в поле.
    fn push_insight(m: &mut SelfModel, text: &str) {
        m.narrative
            .push(crate::entities::self_model::NarrativeSegment {
                id: Uuid::new_v4(),
                text: text.into(),
                created_at: chrono::Utc::now(),
            });
    }

    fn model() -> SelfModel {
        let mut m = SelfModel::new(Uuid::new_v4());
        m.summary = "ценю ясность".into();
        m.add_goal("помочь с проектом");
        m.user_model.perceived_traits = vec!["скептичный".into()];
        push_insight(&mut m, "замечен интерес к Rust");
        m
    }

    #[test]
    fn esc_closes_ctrl_c_quits() {
        let mut s = SelfModelScreen::new(None, Palette::default());
        assert_eq!(
            s.handle_key(key(KeyCode::Esc)),
            Some(SelfModelIntent::Close)
        );
        assert_eq!(
            s.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(SelfModelIntent::Quit)
        );
    }

    #[test]
    fn enter_on_summary_opens_editor_and_commits() {
        let mut s = SelfModelScreen::new(Some(model()), Palette::default());
        // Первая строка — описание себя.
        assert_eq!(s.handle_key(key(KeyCode::Enter)), None);
        assert!(s.editor.is_some());
        // Печать и коммит → намерение правки summary.
        s.handle_key(key(KeyCode::Char('!')));
        let intent = s.handle_key(key(KeyCode::Enter)).unwrap();
        match intent {
            SelfModelIntent::Edit(SelfModelEdit::SetSummary(t)) => assert!(t.contains('!')),
            other => panic!("ожидали SetSummary, получили {other:?}"),
        }
        assert!(s.editor.is_none());
    }

    #[test]
    fn space_cycles_goal_delete_removes() {
        let mut s = SelfModelScreen::new(Some(model()), Palette::default());
        s.handle_key(key(KeyCode::Down)); // на цель
        assert!(matches!(s.selected_action(), Some(RowAction::Goal(_))));
        let cycle = s.handle_key(key(KeyCode::Char(' '))).unwrap();
        assert!(matches!(
            cycle,
            SelfModelIntent::Edit(SelfModelEdit::CycleGoalStatus(_))
        ));
        let del = s.handle_key(key(KeyCode::Delete)).unwrap();
        assert!(matches!(
            del,
            SelfModelIntent::Edit(SelfModelEdit::DeleteGoal(_))
        ));
    }

    #[test]
    fn add_goal_commits_and_empty_is_noop() {
        let mut s = SelfModelScreen::new(None, Palette::default());
        // Строки пустой модели: [Summary, AddGoal, ·spacer·, ·Собеседник·, Traits,
        // Interests, Relationship] — декорации курсор пропускает.
        s.selected = 1; // AddGoal
        assert!(matches!(s.selected_action(), Some(RowAction::AddGoal)));
        s.handle_key(key(KeyCode::Enter));
        assert!(s.editor.is_some());
        // Пустой коммит → no-op.
        assert_eq!(s.handle_key(key(KeyCode::Enter)), None);

        s.handle_key(key(KeyCode::Enter)); // снова открыть
        s.handle_key(key(KeyCode::Char('ц')));
        let intent = s.handle_key(key(KeyCode::Enter)).unwrap();
        assert!(matches!(
            intent,
            SelfModelIntent::Edit(SelfModelEdit::AddGoal(_))
        ));
    }

    #[test]
    fn ctrl_k_confirm_then_clear() {
        let mut s = SelfModelScreen::new(Some(model()), Palette::default());
        assert_eq!(
            s.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL)),
            None
        );
        assert!(s.confirm_clear);
        // Повторный Ctrl+K подтверждает.
        let intent = s
            .handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL))
            .unwrap();
        assert_eq!(intent, SelfModelIntent::Edit(SelfModelEdit::Clear));
        assert!(!s.confirm_clear);
    }

    #[test]
    fn ctrl_k_confirm_cancelled_by_other_key() {
        let mut s = SelfModelScreen::new(Some(model()), Palette::default());
        s.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
        assert!(s.confirm_clear);
        assert_eq!(s.handle_key(key(KeyCode::Down)), None); // отмена
        assert!(!s.confirm_clear);
    }

    #[test]
    fn traits_edit_parses_comma_list() {
        let intent = commit_edit(EditKind::Traits, "  a , b ,, c ".into()).unwrap();
        match intent {
            SelfModelIntent::Edit(SelfModelEdit::SetTraits(v)) => {
                assert_eq!(v, vec!["a".to_string(), "b".into(), "c".into()]);
            }
            other => panic!("ожидали SetTraits, получили {other:?}"),
        }
    }

    #[test]
    fn list_edit_splits_on_newlines_too() {
        // Многострочное поле: элементы можно вводить по одному на строку.
        let intent = commit_edit(EditKind::Interests, "Rust\nратату\n, TUI".into()).unwrap();
        match intent {
            SelfModelIntent::Edit(SelfModelEdit::SetInterests(v)) => {
                assert_eq!(v, vec!["Rust".to_string(), "ратату".into(), "TUI".into()]);
            }
            other => panic!("ожидали SetInterests, получили {other:?}"),
        }
    }

    #[test]
    fn navigation_skips_decoration_rows() {
        // Модель с целью и инсайтом: между AddGoal и Traits — spacer+header,
        // между Relationship и инсайтом — ещё spacer+header. Курсор по `Down`
        // должен перескакивать декорации и не вставать на них.
        let mut s = SelfModelScreen::new(Some(model()), Palette::default());
        let mut seen = Vec::new();
        loop {
            seen.push(s.selected_action().unwrap());
            let before = s.selected;
            s.handle_key(key(KeyCode::Down));
            if s.selected == before {
                break; // достигли конца
            }
        }
        assert!(
            !seen.iter().any(|a| matches!(a, RowAction::Decoration)),
            "курсор не должен стоять на декорациях: {seen:?}"
        );
        // Прошли все выбираемые строки сверху донизу.
        assert!(matches!(seen.first(), Some(RowAction::Summary)));
        assert!(matches!(seen.last(), Some(RowAction::Insight(_))));
    }

    #[test]
    fn render_empty_and_populated_do_not_panic() {
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut empty = SelfModelScreen::new(None, Palette::default());
        term.draw(|f| empty.render(f)).unwrap();
        let mut full = SelfModelScreen::new(Some(model()), Palette::default());
        term.draw(|f| full.render(f)).unwrap();
    }

    #[test]
    fn adjust_scroll_keeps_selection_visible_and_pins_top_of_tall_item() {
        // Выбранная строка целиком в окне — прокрутка не меняется.
        assert_eq!(adjust_scroll(0, 0, 1, 10), 0);
        // Строка выше окна — поднимаем верх окна к её началу.
        assert_eq!(adjust_scroll(5, 2, 1, 10), 2);
        // Низ строки за окном (строка ниже окна) — опускаем окно к её низу.
        assert_eq!(adjust_scroll(0, 9, 1, 5), 5); // sel_end=10, 10-5=5
        // Длинный пункт, не влезающий по высоте: прижимаем к его ВЕРХУ.
        assert_eq!(adjust_scroll(0, 8, 20, 5), 8);
        // Нулевая высота окна — без изменений.
        assert_eq!(adjust_scroll(3, 0, 1, 0), 3);
    }

    #[test]
    fn render_long_trailing_item_in_short_area_does_not_panic() {
        // Длинный последний инсайт в маленьком окне: ранее `List` пропускал бы такой
        // пункт целиком; теперь рисуется его верхняя часть. Проверяем отсутствие
        // паники при переносе на много рядов в тесной высоте.
        let mut m = SelfModel::new(Uuid::new_v4());
        m.summary = "описание".into();
        push_insight(&mut m, &"очень длинное наблюдение ".repeat(40));
        let mut s = SelfModelScreen::new(Some(m), Palette::default());
        let mut term = Terminal::new(TestBackend::new(40, 8)).unwrap();
        term.draw(|f| s.render(f)).unwrap();
        // Перейдём в самый низ (на длинный инсайт) и перерисуем — прокрутка должна
        // показать его верх без паники.
        s.handle_key(key(KeyCode::End));
        term.draw(|f| s.render(f)).unwrap();
    }
}
