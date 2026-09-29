//! Settings screen — the model picker: the provider's own catalogue behind the
//! model row ([docs/research/model-picker.md](../../../docs/research/model-picker.md),
//! stage 4b). Part of the [`super`] module.
//!
//! Deliberately **not** the Choice popup next door. That one reaches an option
//! by cycling to it, one config write per step, which is the wrong shape for a
//! list of 132; and it has no filter, which is the wrong shape for a list of 132
//! for the other reason. What it shares is [`ListScroll`], the one implementation
//! of a list's scroll state (`tools/list_scroll_check.py`).

use crate::shared::api::catalogue::{
    CatalogModel, CatalogueAnswer, CatalogueError, ModelRole, ModelSlot,
};

use super::*;

/// What the picker knows about the catalogue right now.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum PickerStatus {
    /// The request is out (fork F4: it goes out when the picker opens, never
    /// before).
    Fetching,
    /// The provider answered. An **empty** list is an answer too: Anthropic
    /// publishes no embedding model, and a router with nothing loaded lists
    /// nothing.
    Listed,
    /// There is no list, and this is why. The row keeps the text field it has
    /// always been — "type a name by hand" is one key away, as always (N5).
    Failed(CatalogueError),
}

/// The open picker.
pub(super) struct PickerState {
    /// The row that receives the pick.
    pub(super) field: FieldId,
    /// Which catalogue was asked for, and what it was narrowed to.
    pub(super) slot: ModelSlot,
    /// What that slot pointed at when the picker opened
    /// ([`SettingsScreen::slot_source`]): a late answer about another provider
    /// must not land here.
    pub(super) source: String,
    /// The filter line — a 132-entry list needs one.
    pub(super) input: InputBox,
    /// Everything the answer carried, in the order the provider published (or
    /// newest first, where it publishes a date — `catalogue::parse`).
    pub(super) all: Vec<CatalogModel>,
    /// Indices into [`Self::all`] that match the filter.
    pub(super) results: Vec<usize>,
    /// The highlighted row of the rendered list, where **0 is always "type a
    /// name by hand"** and the catalogue starts at 1.
    pub(super) selected: usize,
    pub(super) scroll: ListScroll,
    pub(super) status: PickerStatus,
}

impl PickerState {
    /// The entry the highlight is on: `None` — the "by hand" row.
    fn picked(&self) -> Option<&CatalogModel> {
        let idx = self.selected.checked_sub(1)?;
        self.all.get(*self.results.get(idx)?)
    }

    /// How many rows the list draws, the "by hand" row included.
    pub(super) fn rows(&self) -> usize {
        self.results.len() + 1
    }
}

/// Which catalogue a model row is filled from (`None` — not a row a catalogue
/// can fill: a managed GGUF path, or anything else).
///
/// The speech rows have a list behind them in one mode, the gateway's
/// (docs/research/openrouter-mode.md, fork F7) — the model's, and the two
/// voices' from the same answer, since a voice belongs to a model. In every
/// other speech mode they are the text fields they have always been.
pub(super) fn model_slot(config: &AppConfig, id: FieldId) -> Option<ModelSlot> {
    match id {
        FieldId::XModelName => Some(ModelSlot::Assistant),
        FieldId::IxModelName => Some(ModelSlot::Impersonation),
        FieldId::EModelName => Some(ModelSlot::Embedder),
        FieldId::TtsModelName | FieldId::TtsVoice | FieldId::TtsUserVoice
            if config.tts.mode == TtsMode::OpenRouter =>
        {
            Some(ModelSlot::Speech)
        }
        _ => None,
    }
}

/// Whether the row takes a voice rather than a model.
pub(super) fn is_voice_row(id: FieldId) -> bool {
    matches!(id, FieldId::TtsVoice | FieldId::TtsUserVoice)
}

impl SettingsScreen {
    /// `Enter` on a model row (fork F1(a)): the catalogue when there is one to
    /// show, and today's text editor when there is not.
    ///
    /// Returns `true` when the picker took the key, `false` to let the caller
    /// open the editor as it always did.
    pub(super) fn open_model_picker(&mut self, id: FieldId) -> Option<SettingsIntent> {
        let slot = model_slot(&self.config, id)?;
        let source = self.slot_source(slot);
        let mut input = InputBox::new();
        input.set_single_line(true);
        let cached = self
            .catalogues
            .iter()
            .find(|(s, src, _)| *s == slot && *src == source);
        let (all, status, intent) = match cached {
            Some((_, _, Ok(models))) => (self.offered(id, models), PickerStatus::Listed, None),
            Some((_, _, Err(err))) => (Vec::new(), PickerStatus::Failed(*err), None),
            // Nothing asked for *this* provider yet — and the asking happens
            // here, on the keypress, never when the screen opens (fork F4).
            None => {
                self.asked.retain(|(s, _)| *s != slot);
                self.asked.push((slot, source.clone()));
                (
                    Vec::new(),
                    PickerStatus::Fetching,
                    Some(SettingsIntent::ListModels(slot)),
                )
            }
        };
        let results = (0..all.len()).collect();
        self.picker = Some(PickerState {
            field: id,
            slot,
            source,
            input,
            all,
            results,
            // On the first catalogue row when there is one: the list is the
            // reason the picker opened.
            selected: 1,
            scroll: ListScroll::default(),
            status,
        });
        self.picker_clamp();
        intent
    }

    /// What a slot points at right now — its mode, the address and where the key
    /// comes from, as one string.
    ///
    /// This is the cache key, and the reason for it: a slot's **provider changes
    /// inside one visit** to this screen. Keyed by the slot alone, the first
    /// answer was shown under every mode cycled to afterwards — OpenAI's 132
    /// models offered for `gemini`, `claude` and `grok` (reported from a live run,
    /// 2026-09-18). Where the key comes from is in here too, so correcting a
    /// mistyped variable name asks again instead of repeating "no key".
    pub(super) fn slot_source(&self, slot: ModelSlot) -> String {
        use crate::shared::config::{CloudSettings, ExternalSettings};
        /// An engine slot's address and the variable its key is read from: its
        /// cloud section's, or its `external` one's.
        fn of<'a>(
            cloud: Option<&'a CloudSettings>,
            external: &'a ExternalSettings,
        ) -> (Option<&'a str>, Option<&'a str>) {
            match cloud {
                Some(c) => (c.url.as_deref(), c.api_key_env.as_deref()),
                None => (external.url.as_deref(), external.api_key_env.as_deref()),
            }
        }
        let cfg = &self.config;
        let (mode, (url, env), secret) = match slot {
            ModelSlot::Assistant => (
                format!("{:?}", cfg.engine.mode),
                of(cfg.engine.cloud(), &cfg.engine.external),
                cfg.engine.secret_key(),
            ),
            ModelSlot::Impersonation => (
                format!("{:?}", cfg.impersonation_engine.mode),
                of(
                    cfg.impersonation_engine.cloud(),
                    &cfg.impersonation_engine.external,
                ),
                cfg.impersonation_engine.secret_key(),
            ),
            ModelSlot::Embedder => (
                format!("{:?}", cfg.embed.mode),
                of(cfg.embed.cloud(), &cfg.embed.external),
                cfg.embed.secret_key(),
            ),
            // The model is not part of it: the voices are read out of the one
            // answer for whichever model the row names when the picker opens.
            ModelSlot::Speech => {
                let section = cfg.tts.cloud();
                let url = section.and_then(|c| c.url.as_deref());
                let env = section.and_then(|c| c.api_key_env.as_deref());
                (
                    format!("{:?}", cfg.tts.mode),
                    (url, env),
                    cfg.tts.secret_key(),
                )
            }
        };
        let stored = secret
            .as_ref()
            .is_some_and(|k| self.secrets_present.contains(k));
        format!(
            "{mode}|{}|{}|{stored}",
            url.unwrap_or_default(),
            env.unwrap_or_default()
        )
    }

    /// The answer to [`SettingsIntent::ListModels`], from the orchestrator.
    ///
    /// Kept per slot while the screen is open (N6), so re-opening the picker
    /// costs nothing; `Ctrl+R` inside it is what asks again.
    pub fn set_model_catalogue(&mut self, slot: ModelSlot, models: CatalogueAnswer) {
        // Filed under what was **asked**, not under what the slot points at now:
        // the two differ when the mode was cycled while the answer was in flight,
        // and filing it under the new provider is the defect this key exists to
        // prevent.
        let asked = self
            .asked
            .iter()
            .find(|(s, _)| *s == slot)
            .map(|(_, src)| src.clone())
            .unwrap_or_else(|| self.slot_source(slot));
        self.asked.retain(|(s, _)| *s != slot);
        self.catalogues
            .retain(|(s, src, _)| !(*s == slot && *src == asked));
        self.catalogues.push((slot, asked.clone(), models.clone()));
        let waiting = self
            .picker
            .as_ref()
            .filter(|st| st.slot == slot && st.source == asked)
            .map(|st| st.field);
        let Some(field) = waiting else {
            return;
        };
        let offered = models.map(|list| self.offered(field, &list));
        if let Some(st) = &mut self.picker {
            match offered {
                Ok(list) => {
                    st.all = list;
                    st.status = PickerStatus::Listed;
                }
                Err(err) => {
                    st.all.clear();
                    st.status = PickerStatus::Failed(err);
                }
            }
            st.selected = 1;
        }
        self.picker_filter();
    }

    /// What a row is offered out of a catalogue: the models, or — for a voice
    /// row — the voices of the model the speech slot names. A model that lists
    /// none, or that the catalogue does not hold (a name typed by hand), has
    /// none to offer.
    pub(super) fn offered(&self, field: FieldId, models: &[CatalogModel]) -> Vec<CatalogModel> {
        if !is_voice_row(field) {
            return models.to_vec();
        }
        let section = self.config.tts.cloud();
        let named = section
            .and_then(|c| c.model_name.as_deref())
            .map(str::trim)
            .unwrap_or_default();
        let speaking = models.iter().find(|m| m.id == named);
        speaking
            .map(|m| m.voices.iter().map(|v| CatalogModel::voice(v)).collect())
            .unwrap_or_default()
    }

    /// Whether a voice row has been answered and there is nothing to pick from:
    /// the catalogue is here and the model lists no voice. `Enter` then opens
    /// the editor, as it does on a row whose provider refused — a picker could
    /// only show its one row, "type a name by hand".
    pub(super) fn no_voice_to_pick(&self, id: FieldId) -> bool {
        if !is_voice_row(id) {
            return false;
        }
        // A voice row's list is the speech slot's, and what the slot points at
        // names its mode: in a mode that has no list nothing is filed under it.
        let source = self.slot_source(ModelSlot::Speech);
        self.catalogues.iter().any(|(s, src, answer)| {
            *s == ModelSlot::Speech
                && *src == source
                && answer
                    .as_ref()
                    .is_ok_and(|models| self.offered(id, models).is_empty())
        })
    }

    /// Recomputes the matching entries (case-insensitive, every word must
    /// appear — the search overlay's rule).
    pub(super) fn picker_filter(&mut self) {
        let Some(st) = &mut self.picker else { return };
        let q = st.input.text().to_lowercase();
        let terms: Vec<&str> = q.split_whitespace().collect();
        st.results = st
            .all
            .iter()
            .enumerate()
            .filter(|(_, m)| {
                let hay = format!(
                    "{} {}",
                    m.id.to_lowercase(),
                    m.display.as_deref().unwrap_or_default().to_lowercase()
                );
                terms.iter().all(|t| hay.contains(t))
            })
            .map(|(i, _)| i)
            .collect();
        self.picker_clamp();
    }

    /// Keeps the highlight on a row that exists.
    fn picker_clamp(&mut self) {
        if let Some(st) = &mut self.picker {
            let last = st.rows().saturating_sub(1);
            st.selected = st.selected.min(last);
        }
    }

    pub(super) fn handle_picker_key(&mut self, key: KeyEvent) -> Option<SettingsIntent> {
        let st = self.picker.as_mut()?;
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => {
                self.picker = None;
                None
            }
            (KeyCode::Up, _) => {
                st.selected = st.selected.saturating_sub(1);
                None
            }
            (KeyCode::Down, _) => {
                if st.selected + 1 < st.rows() {
                    st.selected += 1;
                }
                None
            }
            (KeyCode::Home, _) => {
                st.selected = 0;
                None
            }
            (KeyCode::End, _) => {
                st.selected = st.rows().saturating_sub(1);
                None
            }
            (KeyCode::Enter, _) => {
                let picked = st.picked().map(|m| m.id.clone());
                let field = st.field;
                self.picker = None;
                match picked {
                    // The catalogue's id, verbatim: on a multi-model endpoint
                    // this string is what selects the model (N3).
                    Some(id) => self.apply_text(field, &id),
                    // "Type a name by hand" — the editor the row has always had.
                    None => {
                        let value = self
                            .fields()
                            .into_iter()
                            .find(|f| f.id == field)
                            .and_then(|f| match f.kind {
                                FieldKind::Text(v) => Some(v),
                                _ => None,
                            })
                            .unwrap_or_default();
                        self.open_text_editor(field, &value);
                        None
                    }
                }
            }
            // Ask the provider again — the catalogue is otherwise kept for the
            // whole visit to the screen (N6).
            (KeyCode::Char(_), KeyModifiers::CONTROL) => {
                match keys::hotkey_char(&key) {
                    Some('r') => {
                        let (slot, source) = (st.slot, st.source.clone());
                        st.all.clear();
                        st.results.clear();
                        st.status = PickerStatus::Fetching;
                        self.catalogues
                            .retain(|(s, src, _)| !(*s == slot && *src == source));
                        self.asked.retain(|(s, _)| *s != slot);
                        self.asked.push((slot, source));
                        Some(SettingsIntent::ListModels(slot))
                    }
                    // Every other Ctrl combination belongs to the filter's own
                    // input (Ctrl+K clears it).
                    _ => {
                        st.input.on_key(key);
                        self.picker_filter();
                        None
                    }
                }
            }
            _ => {
                st.input.on_key(key);
                self.picker_filter();
                None
            }
        }
    }

    /// Whether this row's provider has already been asked and had nothing to
    /// give — in which case `Enter` opens the editor directly rather than a
    /// picker that could only repeat the refusal (fork F1(a)).
    pub(super) fn catalogue_refused(&self, id: FieldId) -> bool {
        let Some(slot) = model_slot(&self.config, id) else {
            return false;
        };
        let source = self.slot_source(slot);
        self.catalogues
            .iter()
            .any(|(s, src, answer)| *s == slot && *src == source && answer.is_err())
    }

    /// One rendered row of the picker: what the user reads.
    ///
    /// The id first, because the id is what the field takes; then the name the
    /// endpoint published for people, and the day it stops serving the model —
    /// both only when it published them.
    ///
    /// Where the endpoint published **facts** — the window, the price, whether
    /// the model takes tools — those take the name's place: on the gateway that
    /// publishes them the name repeats the id (`Anthropic: Claude Haiku 4.5` for
    /// `anthropic/claude-haiku-4.5`), and the row is one line. The name is still
    /// what the filter searches.
    ///
    /// A **speech** model's row says how many voices it lists and nothing
    /// about its price. The catalogue publishes a number there without its
    /// unit, and the unit differs by model — measured, Grok's is a price per
    /// character (50 characters cost 50 × its `prompt` price) and Gemini's per
    /// token — so "per 1M tokens" beside it would be our claim, and a wrong
    /// one (docs/research/openrouter-mode.md §13).
    pub(super) fn picker_label(&self, m: &CatalogModel) -> String {
        let mut line = m.id.clone();
        if m.role == ModelRole::Speech {
            if !m.voices.is_empty() {
                line.push_str(" · ");
                line.push_str(&self.loc().tf(
                    "ui.settings.models.voices",
                    &[("count", &m.voices.len().to_string())],
                ));
            }
        } else if !m.facts.is_empty() {
            for fact in self.picker_facts(m) {
                line.push_str(" · ");
                line.push_str(&fact);
            }
        } else if let Some(display) = m.display.as_deref().filter(|d| *d != m.id) {
            line.push_str(" — ");
            line.push_str(display);
        }
        if let Some(day) = &m.retiring {
            line.push_str(&format!(
                " · {}",
                self.loc()
                    .tf("ui.settings.models.retiring", &[("date", day)])
            ));
        }
        line
    }
}

impl SettingsScreen {
    /// What the catalogue said about a model, as the row's pieces, in the order
    /// a choice is made in: does it fit, what does it cost, can it act.
    fn picker_facts(&self, m: &CatalogModel) -> Vec<String> {
        let loc = self.loc();
        let mut facts = Vec::new();
        if let Some(window) = m.facts.context_length {
            facts.push(loc.tf(
                "ui.settings.models.window",
                &[("size", &compact_tokens(window))],
            ));
        }
        match (m.role, m.facts.prompt_price, m.facts.completion_price) {
            (_, Some(0), Some(0)) => facts.push(loc.t("ui.settings.models.price_free").to_string()),
            // An embedder answers with vectors: it has a price for what it
            // reads and none for what it writes, which the gateway publishes
            // as `0` — thirty-three rows ending `$0.000 out`.
            (ModelRole::Embedding, Some(input), _) => {
                facts.push(loc.tf("ui.settings.models.price_in", &[("input", &dollars(input))]))
            }
            (_, Some(input), Some(output)) => facts.push(loc.tf(
                "ui.settings.models.price",
                &[("input", &dollars(input)), ("output", &dollars(output))],
            )),
            _ => {}
        }
        if m.facts.tools == Some(false) {
            facts.push(loc.t("ui.settings.models.no_tools").to_string());
        }
        facts
    }
}

/// A token count the way a catalogue row has room for it: `200K`, `1M`,
/// `1.05M`. Rounded **down**, since this is a ceiling somebody will plan
/// against — a window shown larger than it is would be the wrong side to err on.
pub(super) fn compact_tokens(n: u32) -> String {
    if n >= 1_000_000 {
        let hundredths = n / 10_000;
        let (whole, frac) = (hundredths / 100, hundredths % 100);
        match frac {
            0 => format!("{whole}M"),
            f if f % 10 == 0 => format!("{whole}.{}M", f / 10),
            f => format!("{whole}.{f:02}M"),
        }
    } else if n >= 1000 {
        format!("{}K", n / 1000)
    } else {
        n.to_string()
    }
}

/// A price of a million tokens, from millionths of a dollar: two decimals from
/// ten cents up, three below — where two would round a real price to `0.00`.
pub(super) fn dollars(micros: u64) -> String {
    let value = micros as f64 / 1e6;
    if micros >= 100_000 {
        format!("{value:.2}")
    } else {
        format!("{value:.3}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::api::catalogue::ModelRole;

    fn model(id: &str, display: Option<&str>) -> CatalogModel {
        CatalogModel {
            id: id.to_string(),
            display: display.map(str::to_string),
            role: ModelRole::Unstated,
            retiring: None,
            facts: Default::default(),
            voices: Vec::new(),
        }
    }

    #[test]
    fn a_label_carries_the_id_first_and_the_published_extras_after() {
        let s = SettingsScreen::new(AppConfig::default(), vec![], vec![]);
        assert_eq!(s.picker_label(&model("gpt-5.6-sol", None)), "gpt-5.6-sol");
        assert_eq!(
            s.picker_label(&model("claude-opus-5", Some("Claude Opus 5"))),
            "claude-opus-5 — Claude Opus 5"
        );
        assert_eq!(
            s.picker_label(&CatalogModel {
                retiring: Some("2026-10-23".into()),
                ..model("gpt-4", None)
            }),
            format!(
                "gpt-4 · {}",
                s.loc()
                    .tf("ui.settings.models.retiring", &[("date", "2026-10-23")])
            )
        );
    }

    // ---------- a row with facts (docs/research/openrouter-mode.md, fork F7) ----------

    use crate::shared::api::catalogue::ModelFacts;

    /// Runs a table — a number, what is shown for it, `|`, why — through
    /// `shown`, and returns the rows that came out otherwise, each with what
    /// did come out. A row that cannot be read is one of them.
    fn misprinted<N: std::str::FromStr>(table: &str, shown: fn(N) -> String) -> Vec<String> {
        let rows = table.lines().filter(|l| !l.trim().is_empty());
        rows.filter_map(|row| {
            let c: Vec<&str> = row.split_whitespace().collect();
            let got = c[0].parse().map(shown).ok();
            (got.as_deref() != Some(c[1])).then(|| format!("{} => {got:?}", row.trim()))
        })
        .collect()
    }

    /// A window is a ceiling somebody will plan against, so it is cut, never
    /// rounded: `999999` is not `1M`, and Gemini's `1048576` is `1.04M`. What
    /// the cut leaves is written without a trailing zero.
    const WINDOWS: &str = "
        0           0      | nothing to abbreviate
        999         999    | below a thousand, the number itself
        1000        1K     | a thousand
        1999        1K     | cut, not rounded to the nearest
        8191        8K     | an embedder's window
        200000      200K   | Claude Haiku 4.5
        999999      999K   | not 1M: that would be a window it does not have
        1000000     1M     | a million, with no decimals
        1048576     1.04M  | Gemini 3.5 Flash: 1.048 is cut to 1.04
        1050000     1.05M  | two decimals where there are two
        1100000     1.1M   | one where the second is a zero
        1999999     1.99M  | cut below two million
        2000000     2M     | the router's window
        10490000    10.49M | tens of millions
    ";

    #[test]
    fn a_window_is_cut_to_what_the_row_has_room_for_never_rounded_up() {
        let wrong = misprinted(WINDOWS, compact_tokens);
        assert!(wrong.is_empty(), "{wrong:#?}");
    }

    /// A price of a million tokens, from millionths of a dollar: cents from
    /// ten cents up, and a third decimal below — where cents would show thirty
    /// of the gateway's models as `0.0x` and the cheapest as `0.00`.
    const PRICES: &str = "
        1000000   1.00   | Claude Haiku 4.5, a million tokens in
        5000000   5.00   | ...and out
        30000000  30.00  | tens of dollars
        300000    0.30   | Gemini 3.5 Flash
        1236000   1.24   | rounded to the nearest cent
        106000    0.11   | ...also just above ten cents
        100000    0.10   | ten cents: the last price shown in cents
        99000     0.099  | below it, the third decimal appears
        75000     0.075  | seven and a half cents is not 0.07 or 0.08
        20000     0.020  | an embedder: two cents
        1600      0.002  | rounded to the nearest thousandth
        0         0.000  | one side of a price may be free
    ";

    #[test]
    fn a_price_below_ten_cents_keeps_a_third_decimal() {
        let wrong = misprinted(PRICES, dollars);
        assert!(wrong.is_empty(), "{wrong:#?}");
    }

    /// A model the way the gateway's catalogue describes one: a name that
    /// repeats the slug, and facts.
    fn described(id: &str, facts: ModelFacts) -> CatalogModel {
        CatalogModel {
            role: ModelRole::Chat,
            facts,
            ..model(id, Some("A Vendor: A Model Name"))
        }
    }

    fn facts(window: u32, price: Option<(u64, u64)>, tools: Option<bool>) -> ModelFacts {
        ModelFacts {
            context_length: Some(window),
            prompt_price: price.map(|p| p.0),
            completion_price: price.map(|p| p.1),
            tools,
        }
    }

    /// Defect D3 was 460 rows with an id and nothing to choose by. A row now
    /// reads in the order a choice is made in — does it fit, what does it
    /// cost, can it act — as the person choosing reads it, so the assertions
    /// are the rows themselves, in English. The name gives its place to the
    /// facts; a model without tools is **marked**, since this application is
    /// driven by tools, while one with them needs no mark; and the day a
    /// model stops being served still closes the row.
    #[test]
    fn a_row_with_facts_says_whether_it_fits_what_it_costs_and_whether_it_acts() {
        let mut config = AppConfig::default();
        config.interface.language = crate::shared::i18n::Lang::En;
        let s = SettingsScreen::new(config, vec![], vec![]);
        let row = |id: &str, facts: ModelFacts| s.picker_label(&described(id, facts));

        assert_eq!(
            row(
                "anthropic/claude-haiku-4.5",
                facts(200_000, Some((1_000_000, 5_000_000)), Some(true))
            ),
            "anthropic/claude-haiku-4.5 · 200K context · $1.00 in / $5.00 out per 1M tokens"
        );
        assert_eq!(
            row(
                "meta-llama/llama-3.3-70b-instruct:free",
                facts(65_536, Some((0, 0)), Some(false))
            ),
            "meta-llama/llama-3.3-70b-instruct:free · 65K context · free · no tools"
        );
        assert_eq!(
            row("vendor/reads-for-pay", facts(8192, Some((10_000, 0)), None)),
            "vendor/reads-for-pay · 8K context · $0.010 in / $0.000 out per 1M tokens",
            "free on one side is a price, not the word"
        );
        let embedder = CatalogModel {
            role: ModelRole::Embedding,
            ..described("baai/bge-m3", facts(8192, Some((10_000, 0)), None))
        };
        assert_eq!(
            s.picker_label(&embedder),
            "baai/bge-m3 · 8K context · $0.010 per 1M tokens",
            "an embedder writes nothing to price"
        );
        assert_eq!(
            row("openrouter/auto", facts(2_000_000, None, Some(true))),
            "openrouter/auto · 2M context",
            "a router prices itself -1, which is no price to show"
        );
        let leaving = CatalogModel {
            retiring: Some("2026-11-01".into()),
            ..described(
                "openai/gpt-4-turbo",
                facts(128_000, Some((10_000_000, 30_000_000)), None),
            )
        };
        assert_eq!(
            s.picker_label(&leaving),
            "openai/gpt-4-turbo · 128K context · $10.00 in / $30.00 out per 1M tokens \
             · retiring 2026-11-01"
        );
    }

    /// The row from the gateway's own words: one entry of its catalogue,
    /// parsed and drawn. What `parse` keeps in millionths per million tokens
    /// is what the row prints in dollars, so a unit slipped on either side
    /// shows here as a price a thousand times off.
    #[test]
    fn a_catalogue_entry_becomes_the_row_the_gateway_described() {
        use crate::shared::api::catalogue::{CatalogueShape, parse};
        let mut config = AppConfig::default();
        config.interface.language = crate::shared::i18n::Lang::En;
        let s = SettingsScreen::new(config, vec![], vec![]);
        let listed = parse(
            CatalogueShape::OpenRouter,
            r#"{"data":[{"id":"google/gemini-3.5-flash","name":"Google: Gemini 3.5 Flash",
                "created":1779000000,"context_length":1048576,
                "architecture":{"output_modalities":["text"]},
                "pricing":{"prompt":"0.0000003","completion":"0.0000025"},
                "supported_parameters":["max_tokens","temperature"]}]}"#,
        )
        .expect("a catalogue");
        let rows: Vec<String> = listed.iter().map(|m| s.picker_label(m)).collect();
        assert_eq!(
            rows,
            [
                "google/gemini-3.5-flash · 1.04M context · $0.30 in / $2.50 out per 1M tokens \
              · no tools"
            ]
        );
    }
}
