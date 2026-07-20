//! Экран чата — отрисовка экрана чата. Часть модуля [`super`]; разбито из
//! монолита chat.rs (см. docs/history/refactoring-god-objects.md, этап 2).

use super::popups::{render_confirm, render_help, render_suggest};
use super::*;

impl ChatScreen {
    // ---------- отрисовка ----------

    /// Правая подпись титула ленты: модель активного режима движка. Для managed —
    /// «имя.gguf · Nk ctx» (контекст осмыслен); для external/облака — имя облачной/
    /// мульти-модельной модели без ctx. Пусто, пока снимок не получен или модель не
    /// задана.
    pub(super) fn model_meta(&self) -> String {
        use crate::shared::config::ServerMode;
        let Some((cfg, _, _, _)) = &self.settings_snapshot else {
            return String::new();
        };
        let Some(name) = cfg.engine.active_model_name() else {
            return String::new();
        };
        // Для managed контекст осмыслен — добавляем «· Nk ctx».
        let ctx = cfg.engine.managed.context_size;
        if cfg.engine.mode == ServerMode::Managed && ctx > 0 {
            format!("{name} · {}k ctx", (ctx + 512) / 1024)
        } else {
            name
        }
    }

    /// Собирает снимок состояния для строки статуса в одном месте — новый индикатор
    /// добавляет поле сюда, а не расширяет сигнатуры `status_bar::render`/`height`.
    /// `background` передаётся отдельно (владелец-`String` живёт у вызывающего дольше
    /// заимствования).
    fn status_model<'a>(&'a self, background: Option<&'a str>) -> status_bar::StatusModel<'a> {
        status_bar::StatusModel {
            statuses: &self.statuses,
            generating: self.generating,
            tokens: self.gen_tokens,
            context: self.gen_context,
            context_exact: self.gen_context_exact,
            reasoning: self.gen_reasoning,
            mouse_scroll: self.mouse_scroll,
            background,
        }
    }

    pub fn render(&mut self, frame: &mut Frame) {
        let _ = self.maybe_recheck_spelling();

        // Анимация спиннера индикатора индексации (петля рисует каждый тик, пока
        // `is_rag_active()`); делитель замедляет смену кадров до приятного темпа.
        if let Some(banner) = &mut self.rag {
            banner.tick = banner.tick.wrapping_add(1);
        }
        // Анимация спиннера предпросмотра имперсонации (пока `is_impersonating()`).
        if let Some(imp) = &mut self.impersonation {
            imp.tick = imp.tick.wrapping_add(1);
        }

        // Высота ввода/предпросмотра растёт под содержимое с учётом переноса (1–6
        // рядов + рамка). Предпросмотр имперсонации переносит по «ширина минус рамка»
        // (без колонки приглашения), а у поля ввода есть колонка `❯` — поэтому его
        // высоту считаем через `content_rows`, который вычитает и рамку, и `PROMPT_W`
        // (та же ширина текста, что и при отрисовке, иначе поле растёт с опозданием).
        let area_w = frame.area().width;
        // `if let` (а не `match &self.impersonation`): в `else`-ветке нужен `&mut
        // self.input` (`content_rows` кэширует ряды), а заём `&self.impersonation`
        // в неё не продлевается — поля непересекающиеся.
        let content_lines = if let Some(imp) = &self.impersonation {
            visual_line_count(&imp.text, area_w.saturating_sub(2).max(1) as usize)
        } else {
            self.input.content_rows(area_w)
        };
        let input_h = (content_lines.clamp(1, 6) + 2) as u16;
        // Баннер индексации RAG занимает строку только когда активен (иначе 0 —
        // пустой прямоугольник, рендер в него безвреден).
        let banner_h: u16 = if self.rag.is_some() { 1 } else { 0 };
        // Высота статус-бара зависит от ширины: хоткеи переносятся, когда не влезают.
        // Снимок статуса собираем временным через `status_model` в каждом вызове
        // (короткоживущий заём `&self` — не пересекается с `&mut self.feed_view` ниже).
        let background = self.background_hint();
        let status_h = status_bar::height(
            frame.area().width as usize,
            &self.status_model(background.as_deref()),
            &self.palette,
            self.loc,
        );
        let [feed_area, banner_area, input_area, status_area] = Layout::vertical([
            Constraint::Min(3),
            Constraint::Length(banner_h),
            Constraint::Length(input_h),
            Constraint::Length(status_h),
        ])
        .areas(frame.area());

        let title = if self.title.is_empty() {
            "mindfork-rs".to_string()
        } else {
            self.title.clone()
        };
        let meta = self.model_meta();
        self.feed_view.render(
            frame,
            feed_area,
            &title,
            &meta,
            &self.feed,
            &self.palette,
            self.loc,
        );

        if let Some(banner) = &self.rag {
            // Кадры спиннера — из набора глифов палитры (Брайль; компат — ASCII).
            let frames = self.palette.glyphs().spinner;
            let spinner = frames[(banner.tick / 2) % frames.len()];
            let line = Line::from(vec![
                Span::styled(format!("{spinner} RAG: "), self.palette.accent_style()),
                Span::from(banner.text.clone()),
            ]);
            frame.render_widget(line, banner_area);
        }

        status_bar::render(
            frame,
            status_area,
            &self.status_model(background.as_deref()),
            &self.palette,
            self.loc,
        );

        // Во время имперсонации поле ввода скрыто — на его месте потоковый
        // предпросмотр реплики. См. spec §11.8.
        if let Some(imp) = &self.impersonation {
            impersonation_preview::render(
                frame,
                input_area,
                &imp.text,
                imp.tick,
                imp.done,
                &self.palette,
                self.loc,
            );
        } else {
            let input_title = if self.generating {
                self.loc.t("ui.chat.input.generating")
            } else {
                self.loc.t("ui.chat.input.idle")
            };
            let focused = self.profile_overlay.is_none()
                && self.suggest.is_none()
                && self.emoji.is_none()
                && self.confirm.is_none();
            let command = self.input_is_command();
            self.input.render(
                frame,
                input_area,
                crate::widgets::input_box::RenderOpts {
                    title: input_title,
                    focused,
                    command,
                    placeholder: self.loc.t("ui.chat.input.placeholder"),
                },
                &self.palette,
            );
        }

        if let Some(overlay) = &self.profile_overlay {
            overlay.render(frame, frame.area(), &self.palette, self.loc);
        }
        if let Some(popup) = &self.suggest {
            dim_background(frame, &self.palette);
            render_suggest(frame, popup, &self.palette, self.loc);
        }
        if let Some(picker) = &self.emoji {
            dim_background(frame, &self.palette);
            picker.render(frame, frame.area(), &self.palette, self.loc);
        }
        if let Some(action) = self.confirm {
            dim_background(frame, &self.palette);
            render_confirm(frame, action, &self.palette, self.loc);
        }
        if self.show_help {
            dim_background(frame, &self.palette);
            render_help(frame, &mut self.help_scroll, &self.palette, self.loc);
        }
    }
}

/// Число визуальных рядов, которые займёт `text` при переносе по ширине `width`
/// (для расчёта высоты предпросмотра имперсонации). Минимум 1.
pub(super) fn visual_line_count(text: &str, width: usize) -> usize {
    if width == 0 {
        return 1;
    }
    text.split('\n')
        .map(|line| {
            let chars: Vec<char> = line.chars().collect();
            crate::shared::wrap::wrap_ranges(&chars, width).len().max(1)
        })
        .sum::<usize>()
        .max(1)
}

/// Прямоугольник по центру `area` фиксированной ширины/высоты (с клампом).
pub(super) fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let [h] = Layout::horizontal([Constraint::Length(width.min(area.width))])
        .flex(Flex::Center)
        .areas(area);
    let [v] = Layout::vertical([Constraint::Length(height.min(area.height))])
        .flex(Flex::Center)
        .areas(h);
    v
}
