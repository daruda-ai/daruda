//! Isolated file-input states for whole-window visual verification.

use super::super::{
    file_view_pane::{DiffSource, FileOrigin, FileViewMode, PaneFileContent},
    pane::TabEntry,
    pane_tree::PaneLayout,
};
use crate::workspace::Workspace;
use daruda_store::project::PaneCwd;
use gpui::{Context, Point, Window, px};

const SAMPLE_SOURCE: &str = "fn handle_failure(error: Error) {\n    let limit = MAX_RETRIES;\n\n    if error.can_retry() {\n        retry(limit);\n    } else {\n        log_failure(error);\n    }\n}\n";
const SAMPLE_DRAFT: &str = "Keep the failure log concise. ";
const SAMPLE_FILE: &str = "auth.rs";
const MENU_X: f32 = 650.0;
const MENU_Y: f32 = 260.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FileInputShot {
    Selection,
    Menu,
    Inserted,
}

impl Workspace {
    pub(in crate::workspace) fn open_file_input_for_shot(
        &mut self,
        shot: FileInputShot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_lane().is_none() {
            let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            if self.add_project(root, window, cx).is_none() {
                return;
            }
        }
        let Some(root) = self.active_lane().map(|lane| lane.path.clone()) else {
            return;
        };
        let mut targets = Vec::new();
        for name in ["Agent A", "Agent B"] {
            let pane = self.create_agent_chat_pane(
                Some(PaneCwd::Local(root.clone())),
                None,
                daruda_config::AgentDefinition::claude_default().id,
                Some(name.into()),
                window,
                cx,
            );
            let id = pane.id;
            let tab_id = self.alloc_id();
            self.active_runtime_mut().panes.push(pane);
            self.active_runtime_mut().tabs.push(TabEntry {
                id: tab_id,
                layout: PaneLayout::Pane(id),
                last_focused_pane: id,
                user_label: Some(name.into()),
            });
            targets.push(id);
        }
        let mut pane = self.create_file_pane(
            FileOrigin::Reference,
            root.join(SAMPLE_FILE),
            DiffSource::WorkingTree,
            None,
            FileViewMode::Raw,
            window,
            cx,
        );
        let id = pane.id;
        let file = pane.file_content_mut().expect("file pane");
        file.install_content(
            PaneFileContent::LoadedRaw { truncated: false },
            Vec::new(),
            Some(&mut *window),
            cx,
        );
        file.saved_text = SAMPLE_SOURCE.into();
        file.editor_state.update(cx, |state, cx| {
            state.set_value(SAMPLE_SOURCE, window, cx);
            let start = SAMPLE_SOURCE.find("    if").expect("sample selection");
            let end = SAMPLE_SOURCE.rfind("    }").expect("sample end") + "    }".len();
            state.set_selected_range(start..end, cx);
        });
        let tab_id = self.alloc_id();
        self.active_runtime_mut().panes.push(pane);
        self.active_runtime_mut().tabs.push(TabEntry {
            id: tab_id,
            layout: PaneLayout::Pane(id),
            last_focused_pane: id,
            user_label: None,
        });
        self.active_runtime_mut().active_tab_index = self.active_runtime().tabs.len() - 1;
        self.set_focused_pane(id, window, cx);
        self.select_bottom_input_pane(targets[1], window, cx);
        self.input_dock.input.update(cx, |state, cx| {
            state.set_value(SAMPLE_DRAFT, window, cx);
            state.move_cursor_to_end(cx);
        });
        self.activate_bottom_input(cx);
        self.docks.bottom.update(cx, |dock, cx| {
            dock.open();
            cx.notify();
        });
        self.main_area.pending_resize = true;
        let position = Point::new(px(MENU_X), px(MENU_Y));
        match shot {
            FileInputShot::Menu => self.open_file_reference_menu(id, position, window, cx),
            FileInputShot::Inserted => {
                self.add_file_reference(id, true, window, cx);
            }
            FileInputShot::Selection => {
                cx.defer_in(window, move |ws, _window, cx| {
                    if let Some(reference) = ws.file_reference(id, true, cx) {
                        let file = ws.focused_file_content_mut().expect("sample file");
                        file.selection_action = Some(super::FileSelectionAction {
                            reference: std::sync::Arc::new(reference),
                            position,
                            scroll: file.editor_state.read(cx).scroll_offset(),
                        });
                        cx.notify();
                    }
                });
            }
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    // CLI state mapping lives in screenshot_scenario::tests; the states render in captures.
}
