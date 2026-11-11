//! File-viewer references added to the existing bottom input.

mod context;
mod recipient;
mod reference;
mod render;
mod selection;
#[cfg(feature = "screenshot")]
mod shot;
#[cfg(feature = "screenshot")]
pub(crate) use shot::FileInputShot;

pub(in crate::workspace) use context::{append_text, prompt_text};
pub(in crate::workspace) use recipient::FileSendAction;
pub(in crate::workspace) use reference::FileReference;
pub(in crate::workspace) use render::selection_action;
pub(in crate::workspace) use selection::FileSelectionAction;

use super::{
    file_view_pane::{FileViewMode, PaneFileContent},
    pane_tree::PaneId,
};
use crate::{surface::strings as s, workspace::Workspace};
use gpui::{App, Context, Window};
use reference::{SelectedSource, SelectionSource};

impl Workspace {
    pub(in crate::workspace) fn add_focused_file_to_input(
        &mut self,
        selection: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.workspace_page.is_none() && self.focused_file_view().is_some() {
            self.add_file_reference(self.active_runtime().focused_pane_id, selection, window, cx);
        }
    }

    pub(in crate::workspace) fn file_reference(
        &self,
        pane_id: PaneId,
        selection: bool,
        cx: &App,
    ) -> Option<FileReference> {
        let file = self
            .active_runtime()
            .panes
            .iter()
            .find(|p| p.id == pane_id)?
            .file_content()?;
        let view = &file.view;
        let selected = if !selection {
            None
        } else {
            match &view.content {
                PaneFileContent::LoadedRaw { .. } | PaneFileContent::LoadedDiff { .. } => {
                    let editor = file.editor_state.read(cx);
                    let range = editor.selected_range();
                    let text = editor.text().to_string();
                    let source = if view.content.is_loaded_diff() {
                        SelectionSource::Diff(view.active_rows().iter().map(Into::into).collect())
                    } else {
                        SelectionSource::Raw {
                            unsaved: text != file.saved_text,
                        }
                    };
                    Some(SelectedSource {
                        text,
                        range,
                        source,
                    })
                }
                PaneFileContent::LoadedMarkdown { raw_rows, .. }
                    if view.view_mode == FileViewMode::Raw =>
                {
                    view.selection_drag.char_selection().map(|sel| {
                        let (start, end) = sel.ordered();
                        let text = raw_rows
                            .iter()
                            .map(|r| r.content.as_str())
                            .collect::<Vec<_>>()
                            .join("\n");
                        let offset = |row: usize, byte: usize| {
                            raw_rows
                                .iter()
                                .take(row)
                                .map(|r| r.content.len() + 1)
                                .sum::<usize>()
                                + raw_rows.get(row).map_or(0, |r| byte.min(r.content.len()))
                        };
                        SelectedSource {
                            range: offset(start.row, start.byte)..offset(end.row, end.byte),
                            text,
                            source: SelectionSource::Raw { unsaved: false },
                        }
                    })
                }
                _ => None,
            }
        }
        .filter(SelectedSource::has_code);
        Some(FileReference {
            path: view.path.clone(),
            source: view.source.clone(),
            selected,
        })
    }

    pub(in crate::workspace) fn file_input_refusal(&self, cx: &App) -> Option<String> {
        let Some(id) = self.bottom_input_pane() else {
            return Some(s::file_viewer::input_unavailable());
        };
        let pane = self.active_runtime().panes.iter().find(|p| p.id == id)?;
        if let Some(chat) = pane.agent_chat_view() {
            let chat = chat.read(cx);
            if chat.is_read_only()
                || !matches!(chat.cwd(), Some(daruda_store::project::PaneCwd::Local(_)))
            {
                return Some(s::file_viewer::input_unavailable());
            }
        }
        if self.focused_question_bound(cx) {
            return Some(s::file_viewer::input_answer_bound());
        }
        None
    }

    pub(in crate::workspace) fn add_file_reference(
        &mut self,
        pane_id: PaneId,
        selected: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(reference) = self.file_reference(pane_id, selected, cx) {
            self.insert_file_reference(pane_id, reference, window, cx);
        }
    }

    pub(in crate::workspace) fn insert_file_reference(
        &mut self,
        pane_id: PaneId,
        reference: FileReference,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let refusal = self
            .file_input_refusal(cx)
            .or_else(|| {
                (self
                    .file_reference(pane_id, reference.selected.is_some(), cx)
                    .as_ref()
                    != Some(&reference))
                .then(s::file_viewer::input_source_changed)
            })
            .or_else(|| {
                reference
                    .selected
                    .as_ref()
                    .filter(|s| s.excerpt().len() > reference::EXCERPT_LIMIT)
                    .map(|_| s::file_viewer::input_selection_too_large())
            });
        if let Some(reason) = refusal {
            self.report_error(
                daruda_store::observability::error_report::ErrorReport::new(reason)
                    .severity(daruda_store::observability::error_report::ErrorSeverity::Warning)
                    .at(file!(), line!())
                    .build(),
                cx,
            );
            return;
        }
        let Some(recipient) = self.bottom_input_pane() else {
            return;
        };
        let pane = self
            .active_runtime()
            .panes
            .iter()
            .find(|p| p.id == recipient)
            .expect("validated input pane");
        let path = pane
            .cwd()
            .and_then(|cwd| daruda_core::path::strip_root(&reference.path, cwd))
            .unwrap_or_else(|| reference.path.clone());
        let text = reference.format(&reference.path.to_string_lossy(), None);
        if reference.selected.is_some() && text.len() > reference::EXCERPT_LIMIT {
            self.report_error(
                daruda_store::observability::error_report::ErrorReport::new(
                    s::file_viewer::input_selection_too_large(),
                )
                .at(file!(), line!())
                .build(),
                cx,
            );
            return;
        }
        let hide_unchanged = self
            .active_runtime()
            .panes
            .iter()
            .find(|pane| pane.id == pane_id)
            .and_then(|pane| pane.file_view())
            .is_some_and(|view| view.hide_unchanged);
        let token =
            context::FileContext::token(&reference, &path.to_string_lossy(), hide_unchanged);
        self.activate_bottom_input(cx);
        if !self.docks.bottom.read(cx).is_open {
            self.docks.bottom.update(cx, |dock, cx| {
                dock.toggle();
                cx.notify();
            });
        }
        let result = self.input_dock.input.update(cx, |state, cx| {
            let token = token?;
            let cursor = state.cursor();
            let value = state.value();
            let before = value.get(..cursor).and_then(|s| s.chars().next_back());
            let after = value.get(cursor..).and_then(|s| s.chars().next());
            let prefix = if before.is_some_and(|c| !c.is_whitespace()) {
                " "
            } else {
                ""
            };
            let suffix = if after.is_none_or(|c| !c.is_whitespace()) {
                " "
            } else {
                ""
            };
            state.replace_range_with_token(cursor..cursor, token, window, cx)?;
            state.insert(suffix, window, cx);
            let end = state.cursor();
            if !prefix.is_empty() {
                state.set_selected_range(cursor..cursor, cx);
                state.insert(prefix, window, cx);
                state.set_selected_range(end + prefix.len()..end + prefix.len(), cx);
            }
            Ok::<_, Box<dyn std::error::Error>>(())
        });
        if let Err(error) = result {
            self.report_error(
                daruda_store::observability::error_report::ErrorReport::new(
                    s::file_viewer::input_unavailable(),
                )
                .message(error.to_string())
                .at(file!(), line!())
                .build(),
                cx,
            );
            return;
        }
        self.focus_bottom_input(window, cx);
        self.dismiss_file_selection_action(cx);
        self.main_area.pending_resize = true;
        cx.notify();
    }

    pub(in crate::workspace) fn open_input_reference(
        &mut self,
        token: &crate::ui::InlineToken,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(context) = context::FileContext::from_token(token) else {
            return;
        };
        let (mode, line, hide_unchanged) = context.location();
        let expected_path = context.path.clone();
        let expected_source = context.source.clone();
        if let Some(owner) = self.projects.lane_owning(&context.path) {
            self.open_pane_file_view(
                owner,
                context.path,
                context.source,
                mode,
                super::tab_ops::OpenIntent::Enter,
                window,
                cx,
            );
        } else if context.source.is_editable() {
            self.open_reference_file(context.path, window, cx);
        } else {
            self.report_error(
                daruda_store::observability::error_report::ErrorReport::new(
                    s::file_viewer::input_source_changed(),
                )
                .at(file!(), line!())
                .build(),
                cx,
            );
            return;
        }
        if self
            .focused_file_view()
            .is_none_or(|view| view.path != expected_path || view.source != expected_source)
        {
            return;
        }
        self.set_file_view_mode(mode, window, cx);
        if self
            .focused_file_view()
            .is_some_and(|view| view.hide_unchanged != hide_unchanged)
        {
            self.toggle_hide_unchanged_for_pane(self.active_runtime().focused_pane_id, window, cx);
        }
        if let Some(line) = line {
            self.scroll_focused_file_viewer_to_line(line, window, cx);
        }
    }
}

#[cfg(test)]
mod tests;
