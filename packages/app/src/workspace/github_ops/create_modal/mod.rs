//! The Create PR form: title (the last commit's subject until edited),
//! description, base branch and draft. The workspace does the pushing and
//! the `gh` call; this owns the fields and what the form shows meanwhile.

mod render;

use std::sync::Arc;

use gpui::{
    AppContext as _, Context, Entity, FocusHandle, Focusable, SharedString, Subscription,
    WeakEntity, Window,
};

use super::create::CreatePrRequest;
use crate::lane::git::GitRepo;
use crate::surface::strings::git as s;
use crate::ui::{InputEvent, InputState, TextareaState, WindowExt as _};
use crate::workspace::ModalView;
use crate::workspace::Workspace;

/// Visible lines of the description box.
const BODY_ROWS: usize = crate::ui::theme::palette::CREATE_PR_BODY_ROWS;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CreateState {
    Editing,
    Creating,
    Failed(SharedString),
}

pub(crate) struct CreatePrModal {
    panel_focus: FocusHandle,
    title: Entity<InputState>,
    body: Entity<TextareaState>,
    base: Entity<InputState>,
    draft: bool,
    state: CreateState,
    workspace: WeakEntity<Workspace>,
    _subscriptions: [Subscription; 2],
}

impl CreatePrModal {
    pub(super) fn new(
        workspace: WeakEntity<Workspace>,
        repo: Arc<dyn GitRepo>,
        base: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let title = cx.new(|cx| InputState::new(window, cx).placeholder(s::pr_placeholder_title()));
        let body = crate::ui::make_markdown_prose_state(
            "",
            s::pr_placeholder_body(),
            BODY_ROWS,
            window,
            cx,
        );
        let base_value = base.unwrap_or_default();
        let base = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(s::pr_placeholder_base())
                .default_value(base_value)
        });
        let on_input = |state: &Entity<InputState>, window: &mut Window, cx: &mut Context<Self>| {
            cx.subscribe_in(
                state,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { .. } => this.submit(window, cx),
                    InputEvent::Change => this.clear_failure(cx),
                    InputEvent::Focus | InputEvent::Blur => {}
                },
            )
        };
        let _subscriptions = [on_input(&title, window, cx), on_input(&base, window, cx)];

        // The last commit's subject is the usual title; read off the UI
        // thread and used only if the user has not typed one meanwhile.
        cx.spawn_in(window, async move |this, cx| {
            let message = cx
                .background_executor()
                .spawn(async move { repo.head_message() })
                .await;
            let Ok(message) = message else {
                return;
            };
            let subject = message.lines().next().unwrap_or_default().trim().to_owned();
            // SILENT-OK: the form may have closed before the commit was read.
            let _ = this.update_in(cx, |this, window, cx| {
                this.prefill_title(subject, window, cx)
            });
        })
        .detach();

        Self {
            panel_focus: cx.focus_handle(),
            title,
            body,
            base,
            draft: false,
            state: CreateState::Editing,
            workspace,
            _subscriptions,
        }
    }

    fn prefill_title(&mut self, subject: String, window: &mut Window, cx: &mut Context<Self>) {
        if subject.is_empty() || !self.title.read(cx).value().is_empty() {
            return;
        }
        self.title
            .update(cx, |input, cx| input.set_value(subject, window, cx));
    }

    fn clear_failure(&mut self, cx: &mut Context<Self>) {
        if matches!(self.state, CreateState::Failed(_)) {
            self.state = CreateState::Editing;
            cx.notify();
        }
    }

    fn toggle_draft(&mut self, cx: &mut Context<Self>) {
        self.draft = !self.draft;
        cx.notify();
    }

    /// The request the fields describe, or `None` while the title is blank.
    pub(super) fn request(&self, cx: &gpui::App) -> Option<CreatePrRequest> {
        let title = self.title.read(cx).value().trim().to_owned();
        if title.is_empty() {
            return None;
        }
        let base = self.base.read(cx).value().trim().to_owned();
        Some(CreatePrRequest {
            title,
            body: self.body.read(cx).value().to_string(),
            base: (!base.is_empty()).then_some(base),
            draft: self.draft,
        })
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.state == CreateState::Creating {
            return;
        }
        let Some(request) = self.request(cx) else {
            self.state = CreateState::Failed(s::pr_err_title_required().into());
            cx.notify();
            return;
        };
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        self.state = CreateState::Creating;
        cx.notify();
        let me = cx.weak_entity();
        let handle = window.window_handle();
        workspace.update(cx, |ws, cx| ws.create_pr(request, me, handle, cx));
    }

    /// The workspace reports a failure; the form stays open with it.
    pub(super) fn fail(&mut self, message: String, cx: &mut Context<Self>) {
        self.state = CreateState::Failed(message.into());
        cx.notify();
    }

    fn dismiss(&mut self, window: &mut Window, cx: &mut gpui::App) {
        window.close_dialog(cx);
    }
}

impl Focusable for CreatePrModal {
    fn focus_handle(&self, cx: &gpui::App) -> FocusHandle {
        self.title.focus_handle(cx)
    }
}

impl ModalView for CreatePrModal {}

#[cfg(test)]
mod tests;
