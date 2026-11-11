//! The shared input's recipient and the draft transition used by pane focus
//! and the file viewer's input tabs.

use gpui::{Context, Focusable as _, SharedString, Window};

use crate::workspace::Workspace;
use crate::workspace::main_area::pane_tree::PaneId;

impl Workspace {
    pub(in crate::workspace) fn focus_bottom_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus = self.input_dock.input.read(cx).focus_handle(cx);
        if let Some(file) = self.focused_file_content() {
            file.editor_state.clone().update(cx, |state, cx| {
                state.set_selection_focus(Some(focus.clone()), cx)
            });
        }
        focus.focus(window, cx);
    }

    pub(in crate::workspace) fn bottom_input_pane(&self) -> Option<PaneId> {
        let focused = self.active_runtime().focused_pane_id;
        if self.pane_consumes_bottom_input(focused) {
            return Some(focused);
        }
        self.input_dock
            .owner
            .filter(|id| self.pane_consumes_bottom_input(*id))
    }

    pub(in crate::workspace) fn bind_bottom_input(
        &mut self,
        id: PaneId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.pane_consumes_bottom_input(id) || self.input_dock.owner == Some(id) {
            return;
        }
        let current = self.input_dock.input.read(cx).content();
        if let Some(owner) = self.input_dock.owner {
            if current.text().is_empty() {
                self.input_dock.drafts.remove(&owner);
            } else {
                self.input_dock.drafts.insert(owner, current);
            }
        }
        let incoming = self
            .input_dock
            .drafts
            .get(&id)
            .cloned()
            .unwrap_or_else(|| crate::ui::InputContent::new(""));
        self.input_dock.owner = Some(id);
        self.input_dock.input.update(cx, |state, cx| {
            state.set_value(incoming, window, cx);
            state.move_cursor_to_end(cx);
        });
    }

    pub(in crate::workspace) fn select_bottom_input_pane(
        &mut self,
        id: PaneId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.focused_file_view().is_none() || !self.pane_consumes_bottom_input(id) {
            return;
        }
        self.bind_bottom_input(id, window, cx);
        self.activate_bottom_input(cx);
        self.sync_question_composer(window, cx);
        self.refresh_slash_menu(cx);
        self.apply_input_placeholder(window, cx);
        self.adapt_dock_to_input_lines(window, cx);
        self.focus_bottom_input(window, cx);
        cx.notify();
    }

    pub(in crate::workspace) fn bottom_input_tabs(&self) -> Vec<(PaneId, SharedString)> {
        if self.focused_file_view().is_none() {
            return Vec::new();
        }
        let candidates: Vec<_> = self
            .active_runtime()
            .tabs
            .iter()
            .flat_map(|tab| {
                tab.layout.pane_ids().into_iter().filter_map(|id| {
                    let pane = self.active_runtime().panes.iter().find(|p| p.id == id)?;
                    if !self.pane_consumes_bottom_input(id) {
                        return None;
                    }
                    let fallback = match &pane.content {
                        crate::workspace::main_area::pane::PaneContent::Terminal(t) => {
                            t.cached_title.clone()
                        }
                        crate::workspace::main_area::pane::PaneContent::AgentChat(a) => self
                            .mirrors
                            .agents
                            .iter()
                            .find(|agent| agent.id == a.agent_id)
                            .map(|agent| agent.name.clone())
                            .unwrap_or_else(|| a.agent_id.clone())
                            .into(),
                        crate::workspace::main_area::pane::PaneContent::File(_) => return None,
                    };
                    let label = tab.user_label.clone().unwrap_or(fallback);
                    Some((id, label))
                })
            })
            .collect();
        let mut counts = std::collections::HashMap::new();
        for (_, label) in &candidates {
            *counts.entry(label.clone()).or_insert(0usize) += 1;
        }
        candidates
            .into_iter()
            .enumerate()
            .map(|(index, (id, label))| {
                let label = if counts[&label] > 1 {
                    crate::surface::strings::bottom_dock::input_pane_number(&label, index + 1)
                        .into()
                } else {
                    label
                };
                (id, label)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
