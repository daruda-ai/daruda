//! Acquire path references and bounded clipboard images, bound to their pane.

use super::super::pane_tree::PaneId;
use super::attachment_load::{IMAGE_SOURCE_LIMIT, Source, load_sources};
use crate::{surface::strings as s, workspace::Workspace};
use daruda_chat::view::{AgentChatView, PromptDispatch};
use daruda_store::observability::{error_report::ErrorReport, log_writer::LogWriter};
use gpui::{ClipboardEntry, Context, WeakEntity, Window};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::workspace) enum AttachmentSubmitOutcome {
    NotApplicable,
    Accepted,
    Rejected,
}

impl Workspace {
    fn composer_attachments_changed(&mut self, cx: &mut Context<Self>) {
        self.sync_question_marks(cx);
        self.main_area.pending_resize = true;
        cx.notify();
    }
    #[cfg(feature = "screenshot")]
    pub(in crate::workspace) fn open_attachment_draft_for_shot(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_agent_chat_transcript_for_shot(window, cx);
        // Leave spare editor space so attachment-induced collapse is visible.
        self.set_bottom_dock_row_preset(
            crate::workspace::layout::ops::bottom_dock_height_for_rows(
                usize::from(self.mirrors.agent.input_max_rows),
                self.input_dock.font.row_height(),
            ),
            window,
            cx,
        );
        let Some(pane) = self.bottom_input_pane() else {
            return;
        };
        if let Some(view) = self.agent_chat_view(pane).cloned() {
            let result = view.update(cx, |view, cx| view.seed_attachment_draft_for_shot(cx));
            if let Err(error) = result {
                self.attachment_error(error, cx);
            }
            self.composer_attachments_changed(cx);
        }
    }
    pub(in crate::workspace) fn composer_paste_action(&mut self, cx: &mut Context<Self>) {
        if self.paste_composer_attachments(cx) {
            cx.stop_propagation();
        } else {
            cx.propagate();
        }
    }
    pub(in crate::workspace) fn composer_drop(
        &mut self,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pane) = self.bottom_input_pane() else {
            return;
        };
        // An answer is text only: a bound composer takes dropped paths as text.
        if let Some(view) = self
            .agent_chat_view(pane)
            .cloned()
            .filter(|_| !self.focused_question_bound(cx))
        {
            self.load_attachments(
                pane,
                view.downgrade(),
                paths.into_iter().map(Source::Path).collect(),
                cx,
            );
        } else {
            self.insert_composer_paths(&paths, window, cx);
        }
    }

    fn insert_composer_paths(
        &mut self,
        paths: &[PathBuf],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let quoted = self.shell_quoted_paths(paths);
        self.input_dock
            .input
            .update(cx, |state, cx| state.insert(quoted, window, cx));
    }

    pub(in crate::workspace) fn pick_composer_attachments(&mut self, cx: &mut Context<Self>) {
        if self.focused_question_bound(cx) {
            return;
        }
        let Some(pane) = self.bottom_input_pane() else {
            return;
        };
        let Some(view) = self.agent_chat_view(pane).cloned() else {
            return;
        };
        let target = view.downgrade();
        let selection = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: Some(s::agent_chat::attachment_add().into()),
        });
        cx.spawn(async move |ws, cx| {
            let result = selection.await;
            if let Err(error) = ws.update(cx, |ws, cx| match result {
                Ok(Ok(Some(paths))) => ws.load_attachments(
                    pane,
                    target,
                    paths.into_iter().map(Source::Path).collect(),
                    cx,
                ),
                Ok(Err(error)) => ws.attachment_error(error.to_string(), cx),
                _ => {}
            }) {
                LogWriter::log(
                    ErrorReport::new("Attachment picker closed with its workspace")
                        .from_error(error.root_cause())
                        .at(file!(), line!())
                        .build(),
                );
            }
        })
        .detach();
    }

    /// Return whether clipboard attachment content consumed the paste action.
    pub(in crate::workspace) fn paste_composer_attachments(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(pane) = self.bottom_input_pane() else {
            return false;
        };
        let Some(view) = self.agent_chat_view(pane).cloned() else {
            return false;
        };
        // A bound composer pastes as text: its answer cannot carry files.
        if self.focused_question_bound(cx) {
            return false;
        }
        let Some(item) = cx.read_from_clipboard() else {
            return false;
        };
        let paths: Vec<_> =
            crate::workspace::main_area::clipboard_paste_ops::clipboard_paths(&item)
                .into_iter()
                .map(Source::Path)
                .collect();
        // File copies can include an image preview; the original paths take precedence.
        if !paths.is_empty() {
            self.load_attachments(pane, view.downgrade(), paths, cx);
            return true;
        }
        let mut sources = Vec::new();
        for entry in item.into_entries() {
            if let ClipboardEntry::Image(image) = entry {
                if image.bytes().len() > IMAGE_SOURCE_LIMIT {
                    self.attachment_error(s::agent_chat::attachment_image_too_large(), cx);
                    return true;
                }
                sources.push(Source::Image(image.bytes().to_vec()));
            }
        }
        if sources.is_empty() {
            return false;
        }
        self.load_attachments(pane, view.downgrade(), sources, cx);
        true
    }

    fn load_attachments(
        &mut self,
        pane: PaneId,
        target: WeakEntity<AgentChatView>,
        sources: Vec<Source>,
        cx: &mut Context<Self>,
    ) {
        let fallback_paths: Vec<_> = sources
            .iter()
            .filter_map(|source| match source {
                Source::Path(path) => Some(path.clone()),
                Source::Image(_) => None,
            })
            .collect();
        cx.spawn(async move |ws, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { load_sources(sources) })
                .await;
            if let Err(error) = ws.update_in(cx, |ws, window, cx| {
                let Some(view) = target.upgrade() else {
                    return;
                };
                let outcome = result.map_err(|e| e.to_string()).and_then(|attachments| {
                    view.update(cx, |v, cx| v.add_attachments(attachments, cx))
                });
                if let Err(error) = outcome {
                    if !fallback_paths.is_empty() {
                        if ws.input_dock.owner == Some(pane) {
                            ws.insert_composer_paths(&fallback_paths, window, cx);
                        } else {
                            // Preserve the original pane's draft if focus changed during loading.
                            let text = ws.shell_quoted_paths(&fallback_paths);
                            let draft = ws
                                .input_dock
                                .drafts
                                .entry(pane)
                                .or_insert_with(|| crate::ui::InputContent::new(""));
                            let separator = if draft.text().is_empty() { "" } else { " " };
                            *draft = super::super::file_context::append_text(
                                draft,
                                &format!("{separator}{text}"),
                            );
                        }
                        LogWriter::log(
                            ErrorReport::new("Attachment paths inserted as text")
                                .message(error)
                                .severity(
                                    daruda_store::observability::error_report::ErrorSeverity::Info,
                                )
                                .at(file!(), line!())
                                .build(),
                        );
                    } else {
                        ws.attachment_error(error, cx);
                    }
                }
                ws.composer_attachments_changed(cx);
            }) {
                LogWriter::log(
                    ErrorReport::new("Attachment load closed with its workspace")
                        .from_error(error.root_cause())
                        .at(file!(), line!())
                        .build(),
                );
            }
        })
        .detach();
    }

    pub(in crate::workspace) fn remove_composer_attachment(
        &mut self,
        pane: PaneId,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        if let Some(view) = self.agent_chat_view(pane).cloned() {
            view.update(cx, |v, cx| v.remove_attachment(index, cx));
            self.composer_attachments_changed(cx);
        }
    }

    /// Only an explicit composer submission consumes its pane's attachment draft.
    pub(in crate::workspace) fn submit_attached_composer(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) -> AttachmentSubmitOutcome {
        let Some(pane) = self.bottom_input_pane() else {
            return AttachmentSubmitOutcome::NotApplicable;
        };
        let Some(view) = self.agent_chat_view(pane).cloned() else {
            return AttachmentSubmitOutcome::NotApplicable;
        };
        if view.read(cx).draft_attachments().is_empty() {
            return AttachmentSubmitOutcome::NotApplicable;
        }
        let dispatch = match view.update(cx, |v, cx| v.send_composer_prompt(text, cx)) {
            Ok(dispatch) => dispatch,
            Err(error) => {
                self.attachment_error(error, cx);
                return AttachmentSubmitOutcome::Rejected;
            }
        };
        self.composer_attachments_changed(cx);
        let submit_outcome = if matches!(dispatch, PromptDispatch::Queued | PromptDispatch::SentNow)
        {
            AttachmentSubmitOutcome::Accepted
        } else {
            self.attachment_error(s::agent_chat::queue_full().to_string(), cx);
            AttachmentSubmitOutcome::Rejected
        };
        let edge = view.update(cx, |v, cx| v.tick_activity(std::time::Instant::now(), cx));
        if let Some(outcome) = edge {
            self.fire_activity_completion(pane, outcome, cx);
        }
        cx.notify();
        submit_outcome
    }

    fn attachment_error(&mut self, detail: String, cx: &mut Context<Self>) {
        self.report_error(
            ErrorReport::new(s::agent_chat::attachment_failed())
                .message(detail)
                .at(file!(), line!())
                .build(),
            cx,
        );
    }
}
