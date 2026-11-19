//! What tests reach of the Create PR form: building it without a dialog,
//! typing and pressing as the user would, and reading where it stands.

use std::sync::Arc;

use daruda_store::project::LaneRef;
use gpui::{Context, WeakEntity, Window};

use super::{CreatePrModal, CreateState};
use crate::lane::git::GitRepo;
use crate::workspace::Workspace;
use crate::workspace::github_ops::create::Unstacked;
use crate::workspace::github_ops::lane_pr::CreatedPr;

impl CreatePrModal {
    /// [`Self::new`] for workspace tests, which build the form without
    /// opening a dialog.
    pub(in crate::workspace) fn new_for_test(
        workspace: WeakEntity<Workspace>,
        lane: LaneRef,
        repo: Arc<dyn GitRepo>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new(workspace, lane, repo, None, window, cx)
    }

    /// Type `title` and `body` as the user would (Change events included).
    pub(crate) fn type_for_test(
        &mut self,
        title: &str,
        body: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.title
            .update(cx, |input, cx| input.set_value(title, window, cx));
        self.body
            .update(cx, |input, cx| input.set_value(body, window, cx));
    }

    pub(crate) fn fields_for_test(&self, cx: &gpui::App) -> (String, String) {
        (
            self.title.read(cx).value().to_string(),
            self.body.read(cx).value().to_string(),
        )
    }

    /// Choose a template and a base as the buttons do.
    pub(crate) fn choose_for_test(
        &mut self,
        template: &str,
        base: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.use_template(template, window, cx);
        self.choose_base(base.to_owned(), window, cx);
    }

    pub(crate) fn toggle_draft_for_test(&mut self, cx: &mut Context<Self>) {
        self.toggle_draft(cx);
    }

    /// The base and the draft flag as a request would carry them.
    pub(crate) fn request_for_test(&self, cx: &gpui::App) -> (Option<String>, bool) {
        (self.base_value(cx), self.draft)
    }

    pub(crate) fn restored_for_test(&self) -> bool {
        self.restored
    }

    pub(crate) fn discard_for_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.discard_creation(window, cx);
    }

    pub(crate) fn failure(&self) -> Option<&str> {
        match &self.state {
            CreateState::Failed(message) => Some(message.as_ref()),
            CreateState::Editing
            | CreateState::Creating
            | CreateState::Exists(_)
            | CreateState::Unstacked { .. } => None,
        }
    }

    /// What is left of a pull request created but not stacked, and whether
    /// stacking it again is in flight.
    pub(crate) fn unstacked(&self) -> Option<(&Unstacked, bool)> {
        match &self.state {
            CreateState::Unstacked { left, retrying } => Some((left, *retrying)),
            _ => None,
        }
    }

    pub(crate) fn stack_again_for_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.stack_again(window, cx);
    }

    pub(crate) fn existing(&self) -> Option<&CreatedPr> {
        match &self.state {
            CreateState::Exists(created) => Some(created),
            _ => None,
        }
    }
}
