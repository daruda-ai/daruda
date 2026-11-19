//! The Create PR form's fields kept as the user leaves them, per branch,
//! and brought back when the form opens again for it: closing the form,
//! a failed create or a quit loses none of it. A field the user did not
//! touch is not kept, so a template or commit subject read later fills it;
//! one they cleared stays cleared. A default template filled in is not the
//! user's: it is not kept.

use daruda_store::review_drafts::{CreationDraft, CreationOwner};
use gpui::{Context, Window};

use super::summary::TargetRead;
use super::{CreatePrModal, PickedBy, Touched};
use crate::github::DraftStore;

impl CreatePrModal {
    /// Whose branch the fields are kept under, once the target and the
    /// signed-in account are known.
    fn resolve_creation_owner(&mut self, cx: &gpui::App) -> Option<CreationOwner> {
        if self.creation_owner.is_none() {
            self.creation_owner = match self.publication.as_ref().map(|read| &read.target) {
                Some(TargetRead::Pushes {
                    repo: Some(repo),
                    shown,
                }) => DraftStore::creation_owner(repo, &shown.branch, cx),
                _ => None,
            };
        }
        self.creation_owner.clone()
    }

    /// Bring back what was left unsent for the branch, into the fields the
    /// user has not typed in since the form opened. Nothing is written
    /// back here: what was kept stays as it was.
    pub(super) fn restore_creation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(owner) = self.resolve_creation_owner(cx) else {
            return;
        };
        let Some(draft) = DraftStore::creation(&owner, cx) else {
            return;
        };
        let mut restored = false;
        if let Some(title) = draft.title.filter(|_| !self.touched.title) {
            self.title
                .update(cx, |input, cx| input.set_value_silently(title, window, cx));
            self.touched.title = true;
            restored = true;
        }
        if let Some(base) = draft.base.filter(|_| !self.touched.base) {
            self.base
                .update(cx, |input, cx| input.set_value_silently(base, window, cx));
            self.touched.base = true;
            restored = true;
        }
        if !self.touched.draft && draft.draft {
            self.draft = true;
            self.touched.draft = true;
            restored = true;
        }
        if !self.touched.body {
            match (draft.body, draft.template) {
                (Some(body), _) => {
                    self.body
                        .update(cx, |input, cx| input.set_value_silently(body, window, cx));
                    self.touched.body = true;
                    restored = true;
                }
                (None, Some(template)) => {
                    if self.apply_template(&template, PickedBy::User, window, cx) {
                        restored = true;
                    }
                }
                (None, None) => {}
            }
        }
        self.restored = restored;
        cx.notify();
    }

    /// Keep the fields the user touched under the branch; nothing kept
    /// drops what was.
    pub(super) fn keep_creation(&mut self, cx: &mut Context<Self>) {
        // Published: what was kept for the branch was dropped with it, and
        // typing in the form left open does not bring it back.
        if self.is_created() {
            return;
        }
        let Some(owner) = self.resolve_creation_owner(cx) else {
            return;
        };
        let value = |touched: bool, text: String| touched.then_some(text);
        let draft = CreationDraft {
            of: owner.clone(),
            title: value(self.touched.title, self.title.read(cx).value().to_string()),
            body: value(self.touched.body, self.body.read(cx).value().to_string()),
            base: value(self.touched.base, self.base.read(cx).value().to_string()),
            draft: self.touched.draft && self.draft,
            template: self
                .template
                .as_ref()
                .filter(|used| used.by == PickedBy::User && !self.touched.body)
                .map(|used| used.path.clone()),
            saved_at: chrono::Utc::now().timestamp(),
        };
        DraftStore::set_creation(&owner, (!draft.is_empty()).then_some(draft), cx);
    }

    /// Whether what is typed is kept across a close or a restart.
    pub(super) fn creation_keeping(&self, cx: &gpui::App) -> crate::github::DraftKeeping {
        DraftStore::keeping(self.creation_owner.as_ref(), cx)
    }

    /// Drop the kept fields and start the form over for the branch.
    pub(super) fn discard_creation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(owner) = &self.creation_owner {
            DraftStore::set_creation(owner, None, cx);
        }
        self.title
            .update(cx, |input, cx| input.set_value_silently("", window, cx));
        self.body
            .update(cx, |input, cx| input.set_value_silently("", window, cx));
        let base = self.default_base.clone().unwrap_or_default();
        self.base
            .update(cx, |input, cx| input.set_value_silently(base, window, cx));
        self.draft = false;
        self.template = None;
        self.touched = Touched::default();
        self.restored = false;
        self.reset_draft();
        // As when it opened: the last commit's subject, if read by now.
        if let Some(subject) = self.subject.clone() {
            self.prefill_title(subject, window, cx);
        }
        if let Some(default) = self.templates.default.clone() {
            self.apply_template(&default.path, PickedBy::Default, window, cx);
        }
        self.read_against(None, window, cx);
        cx.notify();
    }
}
