//! Settings screen — rendering: the section menu, the subsection tab strip, the field
//! list, the Choice popup, and the search overlay. Part of the [super] module; split
//! out of the settings.rs monolith (see docs/history/refactoring-god-objects.md).

use super::helpers::*;
use super::*;

impl SettingsScreen {
    // ---------- rendering ----------

    /// The palette from the working config copy: theme + terminal compatibility mode.
    pub(super) fn palette(&self) -> Palette {
        Palette::for_theme(self.config.interface.theme)
            .with_compat(self.config.interface.terminal_compat)
    }

    /// The interface locale from the working config copy (axis B, docs/i18n-ui.md).
    pub(super) fn loc(&self) -> &'static crate::shared::i18n::Locale {
        crate::shared::i18n::locale(self.config.interface.language)
    }

    pub fn render(&mut self, frame: &mut Frame) {
        let area = frame.area();
        let palette = self.palette();
        let loc = self.loc();
        // Contextual footer. The hints differ **by focus** — that's what teaches the
        // navigation model, which is otherwise undiscoverable: on the sections only
        // Enter goes in, and inside the pane the arrows only change a value while Esc
        // steps back out. Section-specific extras (Profiles: create/delete) are
        // appended in both states. See docs/history/settings-navigation.md §5.1.
        let on_menu = self.focus == Focus::Menu;
        let mut hints: Vec<(&str, &str)> = if on_menu {
            vec![
                ("Tab/↑↓", loc.t("ui.settings.hint.section")),
                ("Enter", loc.t("ui.settings.hint.enter_fields")),
                ("/", loc.t("ui.settings.hint.search")),
                // The settings screen isn't listed in the `F1` help overlay, so the
                // footer is the only place these are discoverable.
                ("Ctrl+Z/Y", loc.t("ui.settings.hint.undo")),
            ]
        } else {
            vec![
                ("↑↓", loc.t("ui.settings.hint.fields")),
                ("←→", loc.t("ui.settings.hint.choose")),
                ("Enter", loc.t("ui.settings.hint.edit")),
                ("Space", loc.t("ui.settings.hint.toggle")),
                ("Del", loc.t("ui.settings.hint.reset")),
                ("Tab", loc.t("ui.settings.hint.section")),
                ("/", loc.t("ui.settings.hint.search")),
                ("Ctrl+Z/Y", loc.t("ui.settings.hint.undo")),
            ]
        };
        if matches!(self.section(), Section::Profiles | Section::Plugins) {
            hints.push(("Ctrl+N", loc.t("ui.settings.hint.new")));
            hints.push(("Ctrl+D", loc.t("ui.settings.hint.delete")));
        }
        hints.push((
            "Esc",
            if on_menu {
                loc.t("ui.settings.hint.close")
            } else {
                loc.t("ui.settings.hint.to_sections")
            },
        ));
        hints.push(("Ctrl+Q", loc.t("ui.settings.hint.quit")));
        // The hotkey line — below the panel (outside the border), wraps as a grid using
        // the same logic as the chat screen's status bar (right-aligned). Height computed up front.
        let hotkeys = status_bar::hotkey_lines(area.width as usize, &hints, &palette);
        let status_h = (hotkeys.len() as u16).max(1);

        frame.render_widget(Clear, area);
        let [panel_area, status_area] =
            Layout::vertical([Constraint::Min(3), Constraint::Length(status_h)]).areas(area);

        let block = palette.panel(
            format!(
                "{}{}",
                palette.glyphs().settings_icon,
                loc.t("ui.settings.ui.title")
            ),
            true,
        );
        let inner = block.inner(panel_area);
        frame.render_widget(&block, panel_area);
        frame.render_widget(Paragraph::new(hotkeys), status_area);

        let [menu_area, fields_area] =
            Layout::horizontal([Constraint::Length(24), Constraint::Min(20)]).areas(inner);

        // Section counters are tied to the selected modes (from the search index) —
        // the sum matches the number of fields in search.
        let counts = self.section_counts();
        self.render_menu(frame, menu_area, &counts);
        self.render_fields(frame, fields_area);

        // The editor on top — with a real cursor (`InputBox::render` requires `&mut`).
        if let Some(editor) = self.editor.as_mut() {
            // System message/greeting — a large multiline popup with
            // wrapping; other fields — a compact single-line strip. On a validation
            // error the title carries a red message and the editor doesn't close.
            let err = editor.error;
            let base_title = if editor.multiline {
                loc.t("ui.editor.multiline_footer")
            } else {
                loc.t("ui.settings.ui.editor_single")
            };
            let title = match err {
                Some(e) => format!(
                    "{} {e} {}",
                    palette.glyphs().warn,
                    loc.t("ui.settings.ui.esc_cancel")
                ),
                None => base_title.to_string(),
            };
            let popup = if editor.multiline {
                centered_rect(80, 40, multiline_popup_height(area), area)
            } else {
                centered_rect(60, 30, 3, area)
            };
            // A large multiline popup (system message/greeting)
            // dims the background so it doesn't blend in; compact single-line strips —
            // don't (an in-place edit).
            if editor.multiline {
                dim_background(frame, &palette);
            }
            frame.render_widget(Clear, popup);
            editor
                .input
                .render(frame, popup, RenderOpts::focused(&title), &palette);
        }

        // The Choice-field picker popup — on top (the editor/choice are closed while
        // searching).
        if self.choice.is_some() {
            self.render_choice(frame, area, &palette);
        }

        // The field-search overlay — on top of everything (the editor is closed while
        // searching).
        if self.search.is_some() {
            self.render_search(frame, area, &palette);
        }
    }

    /// Draws the Choice-field picker popup: the option list, the current one marked.
    pub(super) fn render_choice(&self, frame: &mut Frame, area: Rect, palette: &Palette) {
        let st = self.choice.as_ref().unwrap();
        // Height = the number of options + a border, but no taller than the screen;
        // width by the longest label (with margin), centered.
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
            .panel(self.loc().t("ui.settings.ui.choice_title"), true)
            .border_style(palette.border_style(true));
        let list = List::new(items)
            .block(block)
            .highlight_style(Style::new().reversed());
        let mut state = ListState::default();
        state.select(Some(st.selected.min(st.options.len().saturating_sub(1))));
        frame.render_stateful_widget(list, popup, &mut state);
    }

    /// Draws the search overlay: the query line + the filtered results.
    pub(super) fn render_search(&mut self, frame: &mut Frame, area: Rect, palette: &Palette) {
        let loc = self.loc();
        let popup = centered_rect(72, 50, (area.height * 3 / 4).max(8), area);
        dim_background(frame, palette);
        frame.render_widget(Clear, popup);

        let [input_area, list_area] =
            Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).areas(popup);

        // A snapshot for the list (select/results) before mutably borrowing input.
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

        let title = loc.tf(
            "ui.settings.ui.search_title",
            &[
                ("found", &results.len().to_string()),
                ("total", &all_len.to_string()),
            ],
        );
        self.search.as_mut().unwrap().input.render(
            frame,
            input_area,
            RenderOpts::focused(&title),
            palette,
        );

        // The results list: "breadcrumb   value" (the value muted).
        let inner_w = list_area.width.saturating_sub(2) as usize;
        let items: Vec<ListItem> = if results.is_empty() {
            vec![ListItem::new(Line::styled(
                loc.t("ui.settings.ui.nothing_found"),
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
            .panel(loc.t("ui.settings.ui.search_footer"), false)
            .border_style(palette.border_style(true));
        let list = List::new(items)
            .block(block)
            .highlight_style(Style::new().reversed());
        let mut state = ListState::default();
        if !results.is_empty() {
            state.select(Some(selected.min(results.len() - 1)));
        }
        frame.render_stateful_widget(list, list_area, &mut state);

        // A scrollbar on the panel's right border, when there are more results than the visible height.
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

    /// The number of editable parameters in a section (for the counter in the left menu).
    ///
    /// Tied to the **currently selected mode** of each server's engine/provider
    /// (managed/external/cloud), and taken from the same index as search
    /// ([`Self::build_search_index`]) — so the sum of section counters matches the
    /// number of fields in search. Fields of all subsections (tab strips) are
    /// enumerated for their current modes; the subsection selector — a navigation
    /// tab, not a parameter — doesn't count (`collect_hits` skips it).
    ///
    /// Test-only — rendering uses [`Self::section_counts`] (one pass over the index
    /// for all sections).
    #[cfg(test)]
    pub(super) fn section_field_count(&self, s: Section) -> usize {
        let target = SECTIONS.iter().position(|&x| x == s).unwrap_or(0);
        self.build_search_index()
            .iter()
            .filter(|h| h.section_idx == target)
            .count()
    }

    /// Parameter counters for all sections in [`SECTIONS`] order, tied to the
    /// selected modes. Derived from the search index ([`Self::build_search_index`])
    /// in one pass — so the sum of section counters is identically equal to the
    /// number of fields in search (the same source of truth).
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
        let loc = self.loc();
        let focused = self.focus == Focus::Menu;
        // Width for the menu row's content (minus the right border) — for right-aligning
        // the field counter.
        let inner_w = area.width.saturating_sub(1) as usize;
        // The active section is marked by a colored rail and a bold title regardless
        // of focus; keyboard selection is highlighted by List's highlight.
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
                    Span::styled(s.title(loc), Style::new().fg(palette.text).bold())
                } else {
                    Span::styled(s.title(loc), palette.muted_style())
                };
                // The section's field counter, right-aligned in the menu.
                let count = counts.get(i).copied().unwrap_or(0).to_string();
                let used = 2 + label_width(s.title(loc)) + count.chars().count();
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
            // The marker is always present — only its colour tracks the focus. It used
            // to appear and disappear, which flickered and shifted the title text.
            .title(Line::from(vec![
                Span::styled(
                    format!(" {} ", palette.glyphs().collapsed),
                    focus_marker_style(focused, &palette),
                ),
                Span::styled(
                    format!("{} ", loc.t("ui.settings.ui.sections")),
                    palette.muted_style(),
                ),
            ]));
        // Selection — a soft backdrop (as in the chat list), not inverting the whole
        // row: reverse video would swap fg↔bg per span independently, which would smear
        // the green rail `▌` over ~1.5 columns (the glyph is a left half-block), and
        // different spans would get different backgrounds (rail/title/counter each their
        // own). A uniform `keycap_bg` + a green rail on top reads cleanly.
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

    /// The server-status chip for the "Model" section's active subsection (assistant →
    /// chat, impersonation → impersonation, embeddings → embeddings).
    pub(super) fn model_server_chip(&self, palette: &Palette) -> Vec<Span<'static>> {
        let loc = self.loc();
        let (status, label) = match self.model_sub {
            ModelTab::Assistant => (&self.statuses.chat, loc.t("ui.settings.chip.chat")),
            ModelTab::Impersonation => (
                &self.statuses.impersonation,
                loc.t("ui.settings.chip.impersonation"),
            ),
            ModelTab::Embeddings => (&self.statuses.embed, loc.t("ui.settings.chip.embeddings")),
            // Speech has no server/probe: clients are stateless, built per call
            // (docs/research/tts.md §8) — there's nothing for a chip to show.
            ModelTab::Tts => return Vec::new(),
        };
        server_status_chip(status, label, loc, palette)
    }

    /// The subsection tab strip for the current section: (tab labels, active one).
    /// `None` — a section with no subsections.
    pub(super) fn subsection_tabs(&self) -> Option<(Vec<&'static str>, usize)> {
        let loc = self.loc();
        match self.section() {
            Section::Model => Some((model_tab_labels(loc), self.model_sub as usize)),
            Section::Sampling => Some((sub_tab_labels(loc), self.sampling_sub as usize)),
            Section::Profiles => Some((sub_tab_labels(loc), self.profile_sub as usize)),
            _ => None,
        }
    }

    pub(super) fn render_fields(&self, frame: &mut Frame, area: Rect) {
        let fields = self.fields();
        let focused = self.focus == Focus::Fields;
        let focused_field = focused.then(|| fields.get(self.field_idx)).flatten();
        let palette = self.palette();

        // The subsection selector (if present in the current field set) is drawn not as
        // a list row but as a tab strip above it. Its position is needed for "focus on tabs".
        let sub_pos = fields.iter().position(|f| is_subsection(f.id));
        let tabs = sub_pos.and(self.subsection_tabs());
        let loc = self.loc();

        // Header: the section title (always) + a tab strip (if there are subsections).
        // The bottom panel (value+description) is always reserved when there are fields,
        // and is as tall as the longest hint of THIS field set needs — a hint clipped
        // mid-sentence is unreadable, while a per-field height would shift the list on
        // every step. The ceiling keeps the list from being squeezed out by a wall of
        // text (an MCP tool's description is arbitrary server text).
        let head_h: u16 = 1 + if tabs.is_some() { 1 } else { 0 };
        let desc_h: u16 = if fields.is_empty() {
            0
        } else {
            let cap = HINT_MAX_ROWS.min((area.height as usize).saturating_sub(head_h as usize) / 3);
            let rows = hint_panel_rows(&fields, area.width as usize, cap, loc) as u16;
            // +1 for the top border; never take the last row away from the list.
            (rows + 1).min(area.height.saturating_sub(head_h + 1))
        };
        let [head_area, list_area, desc_area] = Layout::vertical([
            Constraint::Length(head_h),
            Constraint::Min(1),
            Constraint::Length(desc_h),
        ])
        .areas(area);

        let mut title_spans = vec![
            // The counterpart of the section menu's `▸`: same rule, opposite pane.
            Span::styled(
                format!(" {} ", palette.glyphs().title_marker),
                focus_marker_style(focused, &palette),
            ),
            Span::styled(
                format!("{} ", self.section().title(loc)),
                Style::new().fg(palette.text).bold(),
            ),
        ];
        // The "Model/server" section: the active subsection's server-status chip on the
        // right — edit the engine and see the effect (connecting → ready) without leaving to chat.
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
            head_lines.push(tab_strip_line(&labels, active, on_tabs, &palette));
        }
        frame.render_widget(Paragraph::new(head_lines), head_area);

        // A single value column across the WHOLE section (`section_label_col`): values
        // and inline hints of all groups line up on one vertical (a per-group column
        // "sawtoothed" — each group had its own stop). An overlong label
        // (> LABEL_CAP) doesn't push the column — its value sits locally right
        // after the label. This is also where we count the group's toggles (on/total) for
        // the header counter.
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

        // Fields from the default config — for the "modified" marker (built once).
        let default_fields = self.default_fields();

        // Build the elements: a group header is inserted at the transition to a new
        // non-empty group; `select` — the selected field's position among the elements
        // (headers included) for highlight/scroll. We skip the subsection selector
        // (it's a tab strip): while the cursor is on it, the list has no highlight.
        let inner_w = list_area.width as usize;
        let mut items: Vec<ListItem> = Vec::with_capacity(fields.len() + 8);
        let mut select: Option<usize> = None;
        let mut prev_group: Option<&str> = None;
        for (i, f) in fields.iter().enumerate() {
            if is_subsection(f.id) {
                continue;
            }
            if !f.group.is_empty() && prev_group != Some(f.group) {
                // The "on/total" counter — only for groups with ≥2 toggles (there it's
                // informative; for a single toggle it would duplicate the visible [x]).
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
            // User data (profiles and impersonation personas) has no "default value"
            // to deviate from — comparing it against a default config would mark, say,
            // a chosen impersonation persona as "modified" simply because the default
            // config has no personas at all.
            let modified = !is_profile_field(f.id)
                && default_fields
                    .iter()
                    .find(|d| d.id == f.id)
                    .map(|d| value_text(&d.kind, loc) != value_text(&f.kind, loc))
                    .unwrap_or(false);
            // Width for the value: minus the marker(2)+label+indent and the right margin.
            // A label longer than the column (> LABEL_CAP) shifts the value right —
            // we compute the remainder from its real end, so "…" truncation doesn't lie.
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

        // Selection — a soft backdrop (as in the section menu and the chat list), not
        // inverting the whole row; the selected row's green rail is added in
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

        // A scrollbar when there are more elements than the visible height. Drawn over
        // the settings screen's right border: `fields_area` reaches exactly to it (the
        // panel's inner area), so the `list_area.right()` column IS the border line.
        // The title/tab strip now live in a separate header (not in the list) → the bar
        // spans the full height of `list_area`. Content length — the FULL element count
        // (group headers are rows too).
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
                true, // the settings screen's border is drawn in the focus color
                &palette,
            );
        }

        // The bottom panel: the full value of the selected text field (whole paths,
        // truncated with "…" in the list) + a description hint.
        if desc_h > 1 {
            let content_h = desc_h as usize - 1;
            let w = desc_area.width as usize;
            // The hint has first claim on the panel — the height was reserved for it.
            let mut hint: Vec<Line<'static>> = Vec::new();
            if let Some(f) = focused_field {
                if let Some(text) = f.description.as_deref() {
                    hint.extend(wrap_text(text, palette.muted_style(), w));
                }
                // An expanded explanation for a flagged row, when the row has one
                // (a globally-disabled tool) — so the honest gate is
                // understandable, not just "⊘". Driven by the note rather than by
                // `warn`: that flag is raised for several unrelated reasons.
                if let Some(note) = f.warn_note.as_deref() {
                    hint.extend(wrap_text(note, Style::new().fg(palette.warning), w));
                }
            }
            let mut lines: Vec<Line<'static>> = Vec::new();
            if let Some(f) = focused_field
                && let FieldKind::Text(v) = &f.kind
            {
                let shown = v.trim();
                // Show the full value only for "long" fields (paths, URLs, the
                // system message) — in the list they're truncated with "…". Short
                // values (numbers, host) are already fully visible in the list, no need to duplicate.
                let long =
                    crate::shared::wrap::display_width(&shown.chars().collect::<Vec<_>>()) > 32;
                if !shown.is_empty() && shown != "—" && long {
                    // Cap the preview (a multiline system message can be huge):
                    // it fills what the hint leaves and never grows the panel —
                    // the value is also in the list row above, the hint is only here.
                    let preview: String = shown.chars().take(400).collect();
                    lines = wrap_text(&preview, Style::new().fg(palette.text), w);
                    lines.truncate(content_h.saturating_sub(hint.len()));
                }
            }
            lines.extend(hint);
            // Pre-wrapped above (`Wrap` can't be measured before layout), so every
            // line already fits — no re-wrap here.
            let para = Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(palette.border_style(false)),
            );
            frame.render_widget(para, desc_area);
        }
    }
}
