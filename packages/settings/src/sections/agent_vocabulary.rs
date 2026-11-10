//! Where a catalog row's mode / model pickers get their options.
//!
//! Two sources, consulted **per axis independently**: what the agent last
//! advertised ([`AgentVocabularyCache`], keyed on the row's id and command) and the
//! build-time seed for the adapter the row's command names
//! ([`daruda_config::agent_vocabulary_seed`]). A live list on one axis must
//! not erase the seed on the other.
//!
//! Both keys are editable after the row was built, so the lists are rebuilt in
//! place by [`SettingsView::refresh_agent_row_vocabulary`] rather than at
//! render time — building them in `render` would mean mutating select state
//! mid-paint.

use crate::strings as s;
use daruda_store::agent_vocabulary::{AgentVocabularyCache, VocabEntry};
use daruda_ui::select::{SelectOption, SelectState};
use gpui::{Entity, SharedString, Window};

use super::super::{AgentCatalogItem, AgentCatalogRow, SettingsView};

impl SettingsView {
    /// Rebuild one row's mode/model option lists from the row's current id and
    /// command. Both are editable after the row was constructed, so the lists
    /// are re-sourced here rather than at render time (building them in
    /// `render` would mean mutating the select state mid-paint). The
    /// `SelectState` entities are reused, never replaced, so the row's
    /// subscriptions stay wired to them.
    pub(crate) fn refresh_agent_row_vocabulary(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(AgentCatalogItem::Editable(row)) = self.agent_catalog.get(index) else {
            return;
        };
        let row = row.clone();
        let agent_id = row.id_input.read(cx).value().trim().to_string();
        let command = row.command_input.read(cx).value().trim().to_string();
        let mode = selected_value(&row.default_mode_select, cx);
        let delegated = selected_value(&row.delegated_mode_select, cx);
        let model = selected_value(&row.default_model_select, cx);
        let (mode_options, model_options) = agent_row_vocabulary_options(
            &self.agent_vocabulary,
            &agent_id,
            &command,
            &mode,
            &model,
        );
        set_options(&row.default_mode_select, mode_options, &mode, window, cx);
        set_options(&row.default_model_select, model_options, &model, window, cx);
        let delegated_options =
            agent_row_delegated_options(&self.agent_vocabulary, &agent_id, &command, &delegated);
        set_options(
            &row.delegated_mode_select,
            delegated_options,
            &delegated,
            window,
            cx,
        );
        cx.notify();
    }
}

impl AgentCatalogRow {
    /// The pinned session mode, or `None` for the empty "agent default"
    /// sentinel. The one reading both collect paths share, so neither can
    /// disagree about what "no override" looks like.
    pub(crate) fn default_mode(&self, cx: &gpui::App) -> Option<String> {
        selected_override(&self.default_mode_select, cx)
    }

    /// The mode for phone- and orchestrator-opened sessions as the config
    /// stores it: `None` follows the policy default, `Some("")` follows the
    /// session mode, anything else pins that mode.
    pub(crate) fn delegated_mode(&self, cx: &gpui::App) -> Option<String> {
        let value = selected_value(&self.delegated_mode_select, cx);
        match value.as_ref() {
            DELEGATED_FOLLOWS_SESSION => Some(String::new()),
            _ => selected_override(&self.delegated_mode_select, cx),
        }
    }

    /// The pinned model, same sentinel as [`Self::default_mode`].
    pub(crate) fn default_model(&self, cx: &gpui::App) -> Option<String> {
        selected_override(&self.default_model_select, cx)
    }
}

/// A picker's value as an override — `None` for the empty sentinel that means
/// "let the agent use its own default".
fn selected_override(state: &Entity<SelectState>, cx: &gpui::App) -> Option<String> {
    let value = state.read(cx).selected_value()?.trim().to_string();
    (!value.is_empty()).then_some(value)
}

/// A picker's raw value, empty string when nothing is selected — the shape
/// [`vocabulary_options`] takes as `saved` and [`set_options`] re-selects.
fn selected_value(state: &Entity<SelectState>, cx: &gpui::App) -> SharedString {
    state.read(cx).selected_value().cloned().unwrap_or_default()
}

/// Swap a picker's options and re-select `value`. `set_items` alone leaves the
/// selection pointing at the old list's index, so the value is re-resolved
/// against the new one; [`vocabulary_options`] always carries `value`, so this
/// never clears the row's pick.
fn set_options(
    state: &Entity<SelectState>,
    options: Vec<SelectOption>,
    value: &SharedString,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    state.update(cx, |state, cx| {
        state.set_items(options, window, cx);
        state.set_selected_value(value, window, cx);
    });
}

/// The `(modes, models)` option lists for a row whose id is `agent_id` and
/// whose command is `command`. Cache first, then seed, per axis.
pub(crate) fn agent_row_vocabulary_options(
    vocabulary: &AgentVocabularyCache,
    agent_id: &str,
    command: &str,
    saved_mode: &str,
    saved_model: &str,
) -> (Vec<SelectOption>, Vec<SelectOption>) {
    let seed = daruda_config::agent_vocabulary_seed(command);
    let (seed_modes, seed_models) = match seed.as_ref() {
        Some(seed) => (seed.modes.as_slice(), seed.models.as_slice()),
        None => (&[][..], &[][..]),
    };
    (
        vocabulary_options(
            known_axis(vocabulary.known_modes_for(agent_id, command), seed_modes),
            seed.as_ref().and_then(|seed| seed.default_mode.as_deref()),
            saved_mode,
            MODE_LABELS,
        ),
        vocabulary_options(
            known_axis(vocabulary.known_models_for(agent_id, command), seed_models),
            seed.as_ref().and_then(|seed| seed.default_model.as_deref()),
            saved_model,
            MODEL_LABELS,
        ),
    )
}

/// The delegated picker's value for "Same as session mode", stored as
/// `delegated_mode = ""`. It cannot be `""` itself: that is every picker's
/// "no override" entry, which removes the key and so follows the policy
/// default instead. No adapter names a mode with a control character.
pub(crate) const DELEGATED_FOLLOWS_SESSION: &str = "\u{1}follows-session-mode";

/// The delegated picker's value for a stored `delegated_mode`: `""` for an
/// unset key, [`DELEGATED_FOLLOWS_SESSION`] for a blank one (as
/// [`daruda_config::effective_delegated_mode`] reads it), else the mode.
pub(crate) fn delegated_picker_value(stored: Option<&str>) -> SharedString {
    match stored.map(str::trim) {
        None => SharedString::default(),
        Some("") => SharedString::new_static(DELEGATED_FOLLOWS_SESSION),
        Some(mode) => SharedString::from(mode.to_string()),
    }
}

/// The delegated-mode picker's options: a "no override" entry naming what
/// [`daruda_config::delegated_default`] picks, by the name an approval card
/// gives it; then "Same as session mode", the stated opt-out of that default;
/// then the row's mode vocabulary. An adapter with no policy default already
/// follows the session mode from the first entry, so the opt-out is offered
/// only to keep a stored blank selected and saved as it is.
pub(crate) fn agent_row_delegated_options(
    vocabulary: &AgentVocabularyCache,
    agent_id: &str,
    command: &str,
    saved: &str,
) -> Vec<SelectOption> {
    let seed = daruda_config::agent_vocabulary_seed(command);
    let seed_modes = seed.as_ref().map_or(&[][..], |seed| seed.modes.as_slice());
    let known = vocabulary.known_modes_for(agent_id, command);
    let entries = known_axis(known, seed_modes);
    let policy_default = daruda_config::delegated_default_for_command(command);
    let default_named = match policy_default {
        Some(id) => daruda_config::mode_display_name(known, command, id),
        None => s::settings::agent_field_default_mode(),
    };
    let follows_saved = saved == DELEGATED_FOLLOWS_SESSION;
    let saved = if follows_saved { "" } else { saved };
    let mut options = options_with_sentinel(
        entries,
        s::settings::agent_delegated_mode_default_named(default_named),
        saved,
        mode_label,
    );
    if policy_default.is_some() || follows_saved {
        options.insert(
            1,
            SelectOption::new(
                DELEGATED_FOLLOWS_SESSION,
                s::settings::agent_delegated_mode_follows_session_mode(),
            ),
        );
    }
    options
}

/// `(model, mode)` as a collapsed catalog card names them: the picked value,
/// or what the adapter itself defaults to when the row picks nothing — the
/// same names the pickers show, without their "agent default" framing.
pub(crate) fn agent_row_summary(
    vocabulary: &AgentVocabularyCache,
    agent_id: &str,
    command: &str,
    saved_mode: &str,
    saved_model: &str,
) -> (String, String) {
    let seed = daruda_config::agent_vocabulary_seed(command);
    let (seed_modes, seed_models) = match seed.as_ref() {
        Some(seed) => (seed.modes.as_slice(), seed.models.as_slice()),
        None => (&[][..], &[][..]),
    };
    (
        summary_label(
            known_axis(vocabulary.known_models_for(agent_id, command), seed_models),
            seed.as_ref().and_then(|seed| seed.default_model.as_deref()),
            saved_model,
            model_label,
        ),
        summary_label(
            known_axis(vocabulary.known_modes_for(agent_id, command), seed_modes),
            seed.as_ref().and_then(|seed| seed.default_mode.as_deref()),
            saved_mode,
            mode_label,
        ),
    )
}

/// One axis of [`agent_row_summary`].
fn summary_label(
    entries: &[VocabEntry],
    adapter_default: Option<&str>,
    saved: &str,
    label: fn(&VocabEntry) -> String,
) -> String {
    if !saved.is_empty() {
        return entry_label(entries, saved, label);
    }
    match adapter_default {
        Some(id) => entry_label(entries, id, label),
        None => s::settings::agent_vocabulary_agent_default(),
    }
}

/// The label `id`'s advertised entry shows, or the id itself when nothing
/// names it.
fn entry_label(entries: &[VocabEntry], id: &str, label: fn(&VocabEntry) -> String) -> String {
    entries
        .iter()
        .find(|entry| entry.id == id)
        .map_or_else(|| id.to_string(), label)
}

/// What the agent last advertised on one axis, or the adapter seed until it
/// has advertised anything there.
fn known_axis<'a>(cached: Option<&'a [VocabEntry]>, seeded: &'a [VocabEntry]) -> &'a [VocabEntry] {
    cached.unwrap_or(seeded)
}

/// How one axis names its choices: `choice` for each entry in the picker,
/// `adapter_default` for the entry the "agent default" option points at.
#[derive(Clone, Copy)]
struct AxisLabels {
    choice: fn(&VocabEntry) -> String,
    adapter_default: fn(&VocabEntry) -> String,
}

const MODE_LABELS: AxisLabels = AxisLabels {
    choice: mode_label,
    adapter_default: mode_label,
};

/// The agent-default option names the model the default resolves to, not
/// the default choice itself: "Agent default — Opus 5.5", not
/// "Agent default — Default (Opus 5.5)".
const MODEL_LABELS: AxisLabels = AxisLabels {
    choice: model_label,
    adapter_default: resolved_model_label,
};

fn mode_label(entry: &VocabEntry) -> String {
    entry.name.clone()
}

/// Same label the agent-chat model chip shows for this choice.
fn model_label(entry: &VocabEntry) -> String {
    s::agent_chat::agent_model_choice_label(&entry.id, &entry.name, entry.description.as_deref())
}

/// The concrete model a default-model choice resolves to, when advertised.
fn resolved_model_label(entry: &VocabEntry) -> String {
    daruda_acp::resolved_default_model(&entry.id, entry.description.as_deref())
        .map_or_else(|| model_label(entry), str::to_string)
}

/// The `(agent default[ — name])` entry first, then the vocabulary, then
/// `saved` if the vocabulary does not list it — so a value set before the
/// agent was ever connected is never silently dropped.
fn vocabulary_options(
    entries: &[VocabEntry],
    adapter_default: Option<&str>,
    saved: &str,
    labels: AxisLabels,
) -> Vec<SelectOption> {
    options_with_sentinel(
        entries,
        agent_default_label(entries, adapter_default, labels.adapter_default),
        saved,
        labels.choice,
    )
}

/// The empty "no override" entry labelled `sentinel`, then `entries`, then
/// `saved` when the entries do not list it.
fn options_with_sentinel(
    entries: &[VocabEntry],
    sentinel: String,
    saved: &str,
    choice: fn(&VocabEntry) -> String,
) -> Vec<SelectOption> {
    let mut options = vec![SelectOption::new("", sentinel)];
    options.extend(
        entries
            .iter()
            .map(|entry| SelectOption::new(entry.id.clone(), choice(entry))),
    );
    if !saved.is_empty() && !entries.iter().any(|entry| entry.id == saved) {
        options.push(SelectOption::new(saved.to_string(), saved.to_string()));
    }
    options
}

/// Label for the "no override" entry. It names the adapter's own default when
/// daruda knows it, so picking it is a stated choice rather than a blank.
fn agent_default_label(
    entries: &[VocabEntry],
    adapter_default: Option<&str>,
    label: fn(&VocabEntry) -> String,
) -> String {
    let Some(id) = adapter_default else {
        return s::settings::agent_vocabulary_agent_default();
    };
    s::settings::agent_vocabulary_agent_default_named(entry_label(entries, id, label))
}

#[cfg(test)]
impl SettingsView {
    /// Test-only — install a known vocabulary cache and re-source every row's
    /// pickers from it, so a test never depends on the developer's real
    /// `agent_vocabulary.json`.
    pub(crate) fn set_agent_vocabulary_for_test(
        &mut self,
        vocabulary: AgentVocabularyCache,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        self.agent_vocabulary = vocabulary;
        for index in 0..self.agent_catalog.len() {
            self.refresh_agent_row_vocabulary(index, window, cx);
        }
    }
}

#[cfg(test)]
mod tests;
