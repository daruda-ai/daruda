//! Capturing and invalidating the file selection action and its root menu.

use super::{super::pane_tree::PaneId, FileReference};
use crate::workspace::main_area::pane_menu::{
    adapter::build_popup_menu,
    spec::{Activate, ItemState, MenuEntry, icon_item, item},
};
use crate::{surface::strings as s, workspace::Workspace};
use daruda_store::project::LaneRef;
use gpui::{Context, Focusable as _, Pixels, Point, Window};

#[derive(Clone)]
pub(in crate::workspace) struct FileSelectionAction {
    pub reference: std::sync::Arc<FileReference>,
    pub position: Point<Pixels>,
    pub scroll: Point<Pixels>,
}

impl Workspace {
    fn select_file_context_all(&mut self, pane: PaneId, cx: &mut Context<Self>) {
        if let Some(file) = self
            .active_runtime_mut()
            .panes
            .iter_mut()
            .find(|p| p.id == pane)
            .and_then(|p| p.file_content_mut())
        {
            file.view.select_all();
            cx.notify();
        }
    }

    pub(in crate::workspace) fn schedule_file_selection_action(
        &mut self,
        pane: PaneId,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.defer_in(window, move |ws, _window, cx| {
            ws.show_file_selection_action(pane, position, cx)
        });
    }

    pub(in crate::workspace) fn file_selection_source_changed(
        &mut self,
        lane: LaneRef,
        pane: PaneId,
        cx: &mut Context<Self>,
    ) {
        let changed = self
            .main_area
            .runtimes
            .get_mut(&lane)
            .and_then(|runtime| runtime.panes.iter_mut().find(|p| p.id == pane))
            .and_then(|p| p.file_content_mut())
            .is_some_and(|file| file.selection_action.take().is_some());
        if changed {
            cx.notify();
        }
    }

    pub(in crate::workspace) fn dismiss_file_action_on_escape(
        &mut self,
        event: &gpui::KeyDownEvent,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.key == "escape"
            && self
                .focused_file_content()
                .is_some_and(|file| file.selection_action.is_some())
        {
            self.dismiss_file_selection_action(cx);
            cx.stop_propagation();
        }
    }

    pub(in crate::workspace) fn dismiss_file_selection_action(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        for pane in &mut self.active_runtime_mut().panes {
            if let Some(file) = pane.file_content_mut() {
                changed |= file.selection_action.take().is_some();
            }
        }
        if changed {
            cx.notify();
        }
    }

    pub(in crate::workspace) fn show_file_selection_action(
        &mut self,
        pane: PaneId,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if self.active_runtime().focused_pane_id != pane {
            return;
        }
        let Some(reference) = self
            .file_reference(pane, true, cx)
            .filter(|r| r.selected.is_some())
        else {
            return;
        };
        let Some(file) = self
            .active_runtime_mut()
            .panes
            .iter_mut()
            .find(|p| p.id == pane)
            .and_then(|p| p.file_content_mut())
        else {
            return;
        };
        let editor = file.editor_state.read(cx);
        let scroll = editor.scroll_offset();
        let position = match &reference
            .selected
            .as_ref()
            .expect("selected reference")
            .source
        {
            super::reference::SelectionSource::Raw { .. }
            | super::reference::SelectionSource::Diff(_)
                if matches!(
                    file.view.content,
                    super::super::file_view_pane::PaneFileContent::LoadedRaw { .. }
                        | super::super::file_view_pane::PaneFileContent::LoadedDiff { .. }
                ) =>
            {
                let Some(bounds) = editor.range_to_bounds(&editor.selected_range()) else {
                    return;
                };
                bounds.bottom_right() + Point::new(gpui::px(0.), gpui::px(crate::ui::theme::GAP_SM))
            }
            _ => position,
        };
        file.selection_action = Some(FileSelectionAction {
            reference: std::sync::Arc::new(reference),
            position,
            scroll,
        });
        cx.notify();
    }

    pub(in crate::workspace) fn reconcile_file_selection_action(
        &mut self,
        lane: LaneRef,
        pane: PaneId,
        cx: &mut Context<Self>,
    ) {
        if lane != self.active {
            return;
        }
        let Some(file) = self
            .active_runtime()
            .panes
            .iter()
            .find(|p| p.id == pane)
            .and_then(|p| p.file_content())
        else {
            return;
        };
        let Some(action) = file.selection_action.as_ref() else {
            return;
        };
        let editor = file.editor_state.read(cx);
        let stale = action.scroll != editor.scroll_offset()
            || action.reference.selected.as_ref().is_some_and(|selected| {
                matches!(
                    file.view.content,
                    super::super::file_view_pane::PaneFileContent::LoadedRaw { .. }
                        | super::super::file_view_pane::PaneFileContent::LoadedDiff { .. }
                ) && selected.range != editor.selected_range()
            });
        if stale {
            self.dismiss_file_selection_action(cx);
        }
    }

    pub(in crate::workspace) fn open_file_reference_menu(
        &mut self,
        pane: PaneId,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(reference) = self.file_reference(pane, true, cx) else {
            return;
        };
        let Some(file) = self
            .active_runtime()
            .panes
            .iter()
            .find(|p| p.id == pane)
            .and_then(|p| p.file_content())
        else {
            return;
        };
        let editor_mode = matches!(
            file.view.content,
            super::super::file_view_pane::PaneFileContent::LoadedRaw { .. }
                | super::super::file_view_pane::PaneFileContent::LoadedDiff { .. }
        );
        let editable = editor_mode && file.view.holds_editable_buffer();
        let editor_focus = editor_mode.then(|| file.editor_state.read(cx).focus_handle(cx));
        let copy = reference
            .selected
            .as_ref()
            .map(|s| s.excerpt().to_string())
            .or_else(|| {
                file.view
                    .selection_drag
                    .char_selection()
                    .filter(|s| !s.is_empty())
                    .map(|_| file.view.selected_text_for_copy())
            });
        let enabled = |yes| {
            if yes {
                ItemState::Enabled
            } else {
                ItemState::Disabled(None)
            }
        };
        let mut entries = vec![item(
            s::menu::copy(),
            enabled(copy.is_some()),
            Activate::Clipboard(copy.unwrap_or_default()),
        )];
        if editable {
            entries.extend([
                item(
                    s::menu::cut(),
                    ItemState::Enabled,
                    Activate::Action(Box::new(crate::ui::InputCut)),
                ),
                item(
                    s::menu::paste(),
                    ItemState::Enabled,
                    Activate::Action(Box::new(crate::ui::InputPaste)),
                ),
                item(
                    s::menu::undo(),
                    ItemState::Enabled,
                    Activate::Action(Box::new(crate::ui::InputUndo)),
                ),
                item(
                    s::menu::redo(),
                    ItemState::Enabled,
                    Activate::Action(Box::new(crate::ui::InputRedo)),
                ),
            ]);
        }
        if editor_mode {
            entries.push(item(
                s::menu::select_all(),
                ItemState::Enabled,
                Activate::Action(Box::new(crate::ui::InputSelectAll)),
            ));
        }
        if !editor_mode {
            entries.push(item(
                s::menu::select_all(),
                ItemState::Enabled,
                Activate::Op(Box::new(move |ws, _, cx| {
                    ws.select_file_context_all(pane, cx)
                })),
            ));
        }
        entries.push(item(
            s::file_viewer::copy_abs_path(),
            ItemState::Enabled,
            Activate::Clipboard(reference.path.to_string_lossy().into_owned()),
        ));
        entries.push(MenuEntry::Separator);
        let send = self.file_send_action();
        let refusal = self.file_input_refusal(cx).or_else(|| {
            reference
                .selected
                .as_ref()
                .filter(|s| s.excerpt().len() > super::reference::EXCERPT_LIMIT)
                .map(|_| s::file_viewer::input_selection_too_large())
        });
        let state = refusal.map_or(ItemState::Enabled, |reason| {
            ItemState::Disabled(Some(reason.into()))
        });
        entries.push(icon_item(
            send.label,
            send.icon,
            state,
            Activate::Op(Box::new(move |ws, window, cx| {
                ws.insert_file_reference(pane, reference.clone(), window, cx)
            })),
        ));
        entries.extend(self.file_pane_shared_menu_entries(pane, window, cx));
        let menu = build_popup_menu(entries, editor_focus, cx.entity().downgrade(), window, cx);
        let focus = menu.focus_handle(cx);
        if let Some(file) = self
            .active_runtime()
            .panes
            .iter()
            .find(|p| p.id == pane)
            .and_then(|p| p.file_content())
        {
            file.editor_state
                .clone()
                .update(cx, |state, cx| state.set_selection_focus(Some(focus), cx));
        }
        self.open_context_menu(position, menu, window, cx);
    }
}

#[cfg(test)]
mod tests {
    // End-to-end selection and menu tests live in the parent module's tests.
}
