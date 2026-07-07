//! Экран настроек — отрисовка: меню секций, таб-стрип подсекций, список полей,
//! попап выбора и оверлей поиска. Часть модуля [super]; разбито из монолита
//! settings.rs (см. docs/refactoring-god-objects.md).

use super::helpers::*;
use super::*;

impl SettingsScreen {
    // ---------- отрисовка ----------

    /// Палитра по рабочей копии конфига: тема + режим совместимости терминала.
    pub(super) fn palette(&self) -> Palette {
        Palette::for_theme(self.config.interface.theme)
            .with_compat(self.config.interface.terminal_compat)
    }

    pub fn render(&mut self, frame: &mut Frame) {
        let area = frame.area();
        let palette = self.palette();
        // Контекстный футер: базовые хоткеи + специфичные для секции. В «Профилях» —
        // создание/удаление профиля.
        let mut hints: Vec<(&str, &str)> = vec![
            ("Tab", "секция"),
            ("↑↓", "поля"),
            ("Enter", "правка"),
            ("Space", "тумблер"),
            ("←→", "выбор"),
            ("/", "поиск"),
        ];
        if self.focus == Focus::Fields {
            hints.push(("Del", "сброс"));
        }
        if self.section() == Section::Profiles {
            hints.push(("Ctrl+N", "новый"));
            hints.push(("Ctrl+D", "удалить"));
        }
        hints.push(("Esc", "назад"));
        hints.push(("Ctrl+C", "выход"));
        // Строка хоткеев — под панелью (вне рамки), переносится сеткой по той же
        // логике, что статус-бар экрана чата (прижата вправо). Высоту считаем заранее.
        let hotkeys = status_bar::hotkey_lines(area.width as usize, &hints, &palette);
        let status_h = (hotkeys.len() as u16).max(1);

        frame.render_widget(Clear, area);
        let [panel_area, status_area] =
            Layout::vertical([Constraint::Min(3), Constraint::Length(status_h)]).areas(area);

        let block = palette.panel(format!("{}Настройки", palette.glyphs().settings_icon), true);
        let inner = block.inner(panel_area);
        frame.render_widget(&block, panel_area);
        frame.render_widget(Paragraph::new(hotkeys), status_area);

        let [menu_area, fields_area] =
            Layout::horizontal([Constraint::Length(24), Constraint::Min(20)]).areas(inner);

        // Счётчики секций привязаны к выбранным режимам (из индекса поиска) —
        // сумма совпадает с числом полей в поиске.
        let counts = self.section_counts();
        self.render_menu(frame, menu_area, &counts);
        self.render_fields(frame, fields_area);

        // Редактор поверх — с реальным курсором (InputBox::render требует &mut).
        if let Some(editor) = self.editor.as_mut() {
            // Системное сообщение/приветствие — крупный многострочный попап с
            // переносом; прочие поля — компактная однострочная полоса. При ошибке
            // валидации титул несёт красное сообщение и редактор не закрывается.
            let err = editor.error;
            let base_title = if editor.multiline {
                "правка · Shift+Enter перенос · Enter ок · Esc отмена"
            } else {
                "правка · Enter ок · Esc отмена"
            };
            let title = match err {
                Some(e) => format!("{} {e} · Esc отмена", palette.glyphs().warn),
                None => base_title.to_string(),
            };
            let popup = if editor.multiline {
                centered_rect(80, 40, multiline_popup_height(area), area)
            } else {
                centered_rect(60, 30, 3, area)
            };
            // Крупный многострочный попап (системное сообщение/приветствие)
            // притеняет фон, чтобы не сливаться; компактные однострочные полосы —
            // нет (правка на месте).
            if editor.multiline {
                dim_background(frame, &palette);
            }
            frame.render_widget(Clear, popup);
            editor
                .input
                .render(frame, popup, &title, true, &palette, false);
        }

        // Попап выбора Choice-поля — поверх (при поиске редактор/выбор закрыты).
        if self.choice.is_some() {
            self.render_choice(frame, area, &palette);
        }

        // Оверлей поиска по полям — поверх всего (редактор при поиске закрыт).
        if self.search.is_some() {
            self.render_search(frame, area, &palette);
        }
    }

    /// Рисует попап выбора значения Choice-поля: список вариантов, текущий отмечен.
    pub(super) fn render_choice(&self, frame: &mut Frame, area: Rect, palette: &Palette) {
        let st = self.choice.as_ref().unwrap();
        // Высота = число вариантов + рамка, но не выше экрана; ширина по самой
        // длинной подписи (с запасом), центрирован.
        let want_h = (st.options.len() as u16 + 2).min(area.height.max(3));
        let want_w = st
            .options
            .iter()
            .map(|o| o.chars().count())
            .max()
            .unwrap_or(4) as u16
            + 8;
        let popup = centered_rect_wh(want_w.max(24), want_h.max(3), area);
        dim_background(frame, palette);
        frame.render_widget(Clear, popup);
        let items: Vec<ListItem> = st
            .options
            .iter()
            .enumerate()
            .map(|(i, o)| {
                let mark = if i == st.selected { "› " } else { "  " };
                ListItem::new(Line::from(vec![
                    Span::styled(mark, Style::new().fg(palette.accent)),
                    Span::styled(o.clone(), Style::new().fg(palette.text)),
                ]))
            })
            .collect();
        let block = palette
            .panel("выбор · Enter · Esc", true)
            .border_style(palette.border_style(true));
        let list = List::new(items)
            .block(block)
            .highlight_style(Style::new().reversed());
        let mut state = ListState::default();
        state.select(Some(st.selected.min(st.options.len().saturating_sub(1))));
        frame.render_stateful_widget(list, popup, &mut state);
    }

    /// Рисует оверлей поиска: строка запроса + отфильтрованная выдача.
    pub(super) fn render_search(&mut self, frame: &mut Frame, area: Rect, palette: &Palette) {
        let popup = centered_rect(72, 50, (area.height * 3 / 4).max(8), area);
        dim_background(frame, palette);
        frame.render_widget(Clear, popup);

        let [input_area, list_area] =
            Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).areas(popup);

        // Снимок для списка (селект/выдача) до мутабельного заимствования input.
        let (results, all_len, selected): (Vec<(String, String)>, usize, usize) = {
            let s = self.search.as_ref().unwrap();
            let rows = s
                .results
                .iter()
                .map(|&ai| {
                    let h = &s.all[ai];
                    (h.crumb.clone(), h.value.clone())
                })
                .collect();
            (rows, s.all.len(), s.selected)
        };

        let title = format!("Поиск полей ({}/{})", results.len(), all_len);
        self.search
            .as_mut()
            .unwrap()
            .input
            .render(frame, input_area, &title, true, palette, false);

        // Список результатов: «крошка   значение» (значение приглушённо).
        let inner_w = list_area.width.saturating_sub(2) as usize;
        let items: Vec<ListItem> = if results.is_empty() {
            vec![ListItem::new(Line::styled(
                "  ничего не найдено",
                palette.muted_style(),
            ))]
        } else {
            results
                .iter()
                .map(|(crumb, value)| {
                    let vw = if value.is_empty() {
                        0
                    } else {
                        (value.chars().count() + 2).min(inner_w / 2)
                    };
                    let (crumb_s, cw) = truncate_to_width(crumb, inner_w.saturating_sub(vw + 1));
                    let mut spans = vec![Span::styled(crumb_s, Style::new().fg(palette.text))];
                    if vw > 0 {
                        let (vs, _) = truncate_to_width(value, inner_w.saturating_sub(cw + 2));
                        spans.push(Span::raw("  "));
                        spans.push(Span::styled(vs, palette.muted_style()));
                    }
                    ListItem::new(Line::from(spans))
                })
                .collect()
        };
        let block = palette
            .panel("Enter — перейти · ↑↓ — выбор · Esc — отмена", false)
            .border_style(palette.border_style(true));
        let list = List::new(items)
            .block(block)
            .highlight_style(Style::new().reversed());
        let mut state = ListState::default();
        if !results.is_empty() {
            state.select(Some(selected.min(results.len() - 1)));
        }
        frame.render_stateful_widget(list, list_area, &mut state);

        // Скроллбар на правой рамке панели, когда результатов больше видимой высоты.
        if list_area.height > 2 {
            let bar = Rect {
                x: list_area.x,
                y: list_area.y + 1,
                width: list_area.width,
                height: list_area.height - 2,
            };
            render_scrollbar(
                frame,
                bar,
                results.len(),
                bar.height as usize,
                state.offset(),
                true,
                palette,
            );
        }
    }

    /// Число редактируемых параметров секции (для счётчика в меню слева).
    ///
    /// Привязано к **текущему выбранному режиму** движка/провайдера каждого
    /// сервера (managed/external/облако) и берётся из того же индекса, что и
    /// поиск ([`Self::build_search_index`]) — поэтому сумма счётчиков секций
    /// совпадает с числом полей в поиске. Перечисляются поля всех подсекций
    /// (таб-стрипов) для их текущих режимов; селектор подсекции — навигационный
    /// таб, не параметр, — в счёт не входит (`collect_hits` его пропускает).
    ///
    /// Только для тестов — рендер использует [`Self::section_counts`] (один
    /// проход по индексу для всех секций).
    #[cfg(test)]
    pub(super) fn section_field_count(&self, s: Section) -> usize {
        let target = SECTIONS.iter().position(|&x| x == s).unwrap_or(0);
        self.build_search_index()
            .iter()
            .filter(|h| h.section_idx == target)
            .count()
    }

    /// Счётчики параметров всех секций в порядке [`SECTIONS`], привязанные к
    /// выбранным режимам. Выводятся из индекса поиска ([`Self::build_search_index`])
    /// одним проходом — так сумма счётчиков секций тождественно равна числу полей
    /// в поиске (тот же источник истины).
    pub(super) fn section_counts(&self) -> Vec<usize> {
        let mut counts = vec![0usize; SECTIONS.len()];
        for h in self.build_search_index() {
            if let Some(c) = counts.get_mut(h.section_idx) {
                *c += 1;
            }
        }
        counts
    }

    pub(super) fn render_menu(&self, frame: &mut Frame, area: Rect, counts: &[usize]) {
        let palette = self.palette();
        let focused = self.focus == Focus::Menu;
        // Ширина под содержимое строки меню (минус правая рамка) — для правого
        // выравнивания счётчика полей.
        let inner_w = area.width.saturating_sub(1) as usize;
        // Активная секция помечается цветным рейлом и насыщенным заголовком вне
        // зависимости от фокуса; выбор клавиатурой подсвечивает List highlight.
        let items: Vec<ListItem> = SECTIONS
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let active = i == self.section_idx;
                let bar = if active {
                    Span::styled("▌ ", Style::new().fg(palette.success))
                } else {
                    Span::styled("  ", Style::new())
                };
                let title = if active {
                    Span::styled(s.title(), Style::new().fg(palette.text).bold())
                } else {
                    Span::styled(s.title(), palette.muted_style())
                };
                // Счётчик полей секции, прижатый к правому краю меню.
                let count = counts.get(i).copied().unwrap_or(0).to_string();
                let used = 2 + label_width(s.title()) + count.chars().count();
                let pad = inner_w.saturating_sub(used).max(1);
                ListItem::new(Line::from(vec![
                    bar,
                    title,
                    Span::raw(" ".repeat(pad)),
                    Span::styled(count, palette.muted_style()),
                ]))
            })
            .collect();
        let block = Block::default()
            .borders(Borders::RIGHT)
            .border_style(palette.border_style(false))
            .title(Span::styled(
                if focused {
                    format!(" {} Секции ", palette.glyphs().collapsed)
                } else {
                    " Секции ".to_string()
                },
                palette.muted_style(),
            ));
        // Выделение — мягкая подложка (как в списке чатов), а не инверсия всей
        // строки: реверс свапал бы fg↔bg у каждого спана по отдельности, из-за чего
        // зелёный рейл `▌` расползался на ~1.5 колонки (глиф — левый полублок), а
        // разные спаны получали разный фон (рейл/заголовок/счётчик — свой). Единый
        // `keycap_bg` + зелёный рейл поверх него читаются чисто.
        let hl = if focused {
            Style::new().bg(palette.keycap_bg)
        } else {
            Style::new()
        };
        let list = List::new(items).block(block).highlight_style(hl);
        let mut state = ListState::default();
        state.select(Some(self.section_idx));
        frame.render_stateful_widget(list, area, &mut state);
    }

    /// Чип статуса сервера активной подсекции секции «Модель» (ассистент → чат,
    /// имперсонация → имперсонация, эмбеддинги → эмбеддинги).
    pub(super) fn model_server_chip(&self, palette: &Palette) -> Vec<Span<'static>> {
        let (status, label) = match self.model_sub {
            ModelTab::Assistant => (&self.statuses.chat, "чат"),
            ModelTab::Impersonation => (&self.statuses.impersonation, "имперсонация"),
            ModelTab::Embeddings => (&self.statuses.embed, "эмбеддинги"),
        };
        server_status_chip(status, label, palette)
    }

    /// Таб-стрип подсекции для текущей секции: (подписи вкладок, активная).
    /// `None` — секция без подсекций.
    pub(super) fn subsection_tabs(&self) -> Option<(&'static [&'static str], usize)> {
        match self.section() {
            Section::Model => Some((&MODEL_TABS, self.model_sub as usize)),
            Section::Sampling => Some((&SUB_TABS, self.sampling_sub as usize)),
            Section::Profiles => Some((&SUB_TABS, self.profile_sub as usize)),
            _ => None,
        }
    }

    pub(super) fn render_fields(&self, frame: &mut Frame, area: Rect) {
        let fields = self.fields();
        let focused = self.focus == Focus::Fields;
        let focused_field = focused.then(|| fields.get(self.field_idx)).flatten();
        let palette = self.palette();

        // Селектор подсекции (если есть в текущем наборе полей) рисуется не строкой
        // списка, а таб-стрипом над ним. Его позиция нужна для «фокуса на вкладках».
        let sub_pos = fields.iter().position(|f| is_subsection(f.id));
        let tabs = sub_pos.and(self.subsection_tabs());

        // Шапка: титул секции (всегда) + таб-стрип (если есть подсекции). Нижняя
        // панель (значение+описание) резервируется всегда при наличии полей.
        let desc_h: u16 = if fields.is_empty() { 0 } else { 4 };
        let head_h: u16 = 1 + if tabs.is_some() { 1 } else { 0 };
        let [head_area, list_area, desc_area] = Layout::vertical([
            Constraint::Length(head_h),
            Constraint::Min(1),
            Constraint::Length(desc_h),
        ])
        .areas(area);

        let mut title_spans = vec![
            Span::styled(
                format!(" {} ", palette.glyphs().title_marker),
                Style::new().fg(palette.assistant),
            ),
            Span::styled(
                format!("{} ", self.section().title()),
                Style::new().fg(palette.text).bold(),
            ),
        ];
        // Секция «Модель/сервер»: чип статуса сервера активной подсекции справа —
        // правишь движок и видишь эффект (подключение → готов), не выходя в чат.
        if self.section() == Section::Model {
            let chip = self.model_server_chip(&palette);
            let used_left: usize = title_spans.iter().map(|s| span_width(s)).sum();
            let used_right: usize = chip.iter().map(|s| span_width(s)).sum();
            let head_w = head_area.width as usize;
            if head_w > used_left + used_right + 1 {
                title_spans.push(Span::raw(" ".repeat(head_w - used_left - used_right - 1)));
                title_spans.extend(chip);
            }
        }
        let mut head_lines = vec![Line::from(title_spans)];
        if let Some((labels, active)) = tabs {
            let on_tabs = focused && sub_pos == Some(self.field_idx);
            head_lines.push(tab_strip_line(labels, active, on_tabs, &palette));
        }
        frame.render_widget(Paragraph::new(head_lines), head_area);

        // Единая колонка значений на ВСЮ секцию (`section_label_col`): значения и
        // инлайн-подсказки всех групп стоят на одной вертикали (колонка на группу
        // «пилила» — у каждой группы был свой стоп). Сверхдлинная подпись
        // (> LABEL_CAP) колонку не отгоняет — её значение локально встаёт сразу
        // после подписи. Здесь же считаем тумблеры группы (вкл/всего) для счётчика
        // в заголовке.
        let label_col = section_label_col(&fields);
        let mut group_toggles: HashMap<&str, (usize, usize)> = HashMap::new();
        for f in &fields {
            if is_subsection(f.id) {
                continue;
            }
            if let FieldKind::Toggle(on) = f.kind {
                let e = group_toggles.entry(f.group).or_insert((0, 0));
                e.1 += 1;
                if on {
                    e.0 += 1;
                }
            }
        }

        // Поля из дефолтного конфига — для маркера «изменено» (строим один раз).
        let default_fields = self.default_fields();

        // Строим элементы: заголовок группы вставляется на переходе к новой
        // непустой группе; `select` — позиция выбранного поля среди элементов (с
        // учётом заголовков) для подсветки/скролла. Селектор подсекции пропускаем
        // (он — таб-стрип): когда курсор на нём, список без выделения.
        let inner_w = list_area.width as usize;
        let mut items: Vec<ListItem> = Vec::with_capacity(fields.len() + 8);
        let mut select: Option<usize> = None;
        let mut prev_group: Option<&str> = None;
        for (i, f) in fields.iter().enumerate() {
            if is_subsection(f.id) {
                continue;
            }
            if !f.group.is_empty() && prev_group != Some(f.group) {
                // Счётчик «вкл/всего» — только для групп с ≥2 тумблерами (там он
                // информативен; для одиночного тумблера дублировал бы видимый [x]).
                let count = group_toggles
                    .get(f.group)
                    .copied()
                    .filter(|&(_, total)| total >= 2);
                items.push(ListItem::new(header_line(
                    f.group, count, inner_w, &palette,
                )));
            }
            prev_group = Some(f.group);
            if focused && i == self.field_idx {
                select = Some(items.len());
            }
            let modified = default_fields
                .iter()
                .find(|d| d.id == f.id)
                .map(|d| value_text(&d.kind) != value_text(&f.kind))
                .unwrap_or(false);
            // Ширина под значение: минус маркер(2)+подпись+отступ и правый зазор.
            // Подпись длиннее колонки (> LABEL_CAP) сдвигает значение вправо —
            // считаем остаток от её реального конца, чтобы усечение «…» не врало.
            let start = label_col.max(label_width(&f.label));
            let value_w = inner_w.saturating_sub(start + 4);
            items.push(ListItem::new(render_field_line(
                f,
                label_col,
                value_w,
                modified,
                focused && i == self.field_idx,
                &palette,
            )));
        }
        let total = items.len();

        // Выделение — мягкая подложка (как в меню секций и списке чатов), а не
        // инверсия всей строки; зелёный рейл выбранной строки добавлен в
        // `render_field_line`.
        let hl = if focused {
            Style::new().bg(palette.keycap_bg)
        } else {
            Style::new()
        };
        let list = List::new(items)
            .block(Block::default().borders(Borders::NONE))
            .highlight_style(hl);
        let mut state = ListState::default();
        if let Some(sel) = select {
            state.select(Some(sel));
        }
        frame.render_stateful_widget(list, list_area, &mut state);

        // Скроллбар, когда элементов больше видимой высоты. Рисуем поверх правой
        // рамки экрана настроек: `fields_area` доходит ровно до неё (inner панели),
        // поэтому колонка `list_area.right()` — это линия рамки. Титул/таб-стрип
        // теперь в отдельной шапке (не в списке) → бар на всю высоту `list_area`.
        // Длина содержимого — ПОЛНОЕ число элементов (заголовки групп тоже строки).
        if list_area.height > 0 {
            let bar = Rect {
                width: list_area.width + 1,
                ..list_area
            };
            render_scrollbar(
                frame,
                bar,
                total,
                list_area.height as usize,
                state.offset(),
                true, // рамка экрана настроек рисуется в фокусном цвете
                &palette,
            );
        }

        // Нижняя панель: полное значение выбранного текстового поля (пути целиком,
        // в списке они усечены «…») + описание-подсказка.
        if desc_h > 0 {
            let mut lines: Vec<Line<'static>> = Vec::new();
            if let Some(f) = focused_field {
                if let FieldKind::Text(v) = &f.kind {
                    let shown = v.trim();
                    // Полное значение показываем только для «длинных» полей (пути, URL,
                    // системное сообщение) — в списке они усекаются «…». Короткие
                    // значения (числа, host) в списке видны целиком, дублировать незачем.
                    let long =
                        crate::shared::wrap::display_width(&shown.chars().collect::<Vec<_>>()) > 32;
                    if !shown.is_empty() && shown != "—" && long {
                        // Ограничиваем превью (многострочное системное сообщение
                        // может быть огромным) — панель всё равно клипует по высоте.
                        let preview: String = shown.chars().take(400).collect();
                        lines.push(Line::styled(preview, Style::new().fg(palette.text)));
                    }
                }
                if let Some(text) = field_description(f.id) {
                    lines.push(Line::styled(text, palette.muted_style()));
                }
                // Выключенный глобально инструмент — развёрнутое пояснение (цветом
                // предупреждения), чтобы честный гейт был понятен, а не только «⊘».
                if f.warn {
                    lines.push(Line::styled(
                        "Инструмент включён в профиле, но выключен глобальным \
                         выключателем — он недоступен модели. Включите его в секции \
                         «Инструменты».",
                        Style::new().fg(palette.warning),
                    ));
                }
            }
            let para = Paragraph::new(lines)
                .block(
                    Block::default()
                        .borders(Borders::TOP)
                        .border_style(palette.border_style(false)),
                )
                .wrap(Wrap { trim: true });
            frame.render_widget(para, desc_area);
        }
    }
}
