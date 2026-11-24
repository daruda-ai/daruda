//! The Description tab's state beyond the pull request itself: its
//! conversation (comments on the pull request, apart from review threads),
//! read when the tab is first shown, and editing the description, for a
//! viewer GitHub lets edit it.

use daruda_github::{Conversation, GhError};
use daruda_store::observability::error_report::{ErrorReport, ErrorSeverity};
use gpui::{Context, Entity, Subscription, Window};

use super::read_slot::{ReadSlot, ReadTicket};
use super::{Load, PrDetailId, PrDetailState};
use crate::gh_host::GhHost;
use crate::github::{DraftStore, PrPoller};
use crate::surface::strings::pull_requests as s;
use crate::ui::{InputEvent, TextareaState};
use crate::workspace::Workspace;
use crate::workspace::spawn_helpers::spawn_bg_work_and_mutate;
use daruda_store::review_drafts::{DescriptionDraft, DraftOwner};

/// Where saving an edited description stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) enum SaveState {
    Editing,
    /// The description changed on GitHub since editing began: saving
    /// would replace that change, so it waits for a deliberate overwrite.
    Conflict,
    Saving,
    /// What `gh` said; the text stays to save again. Saving the same text
    /// twice changes nothing, so trying again is safe.
    Failed(String),
}

/// What a save found on GitHub.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BodySave {
    Saved,
    /// Changed since editing began: not replaced.
    Moved,
}

/// The description being edited.
pub(in crate::workspace) struct BodyEdit {
    pub input: Entity<TextareaState>,
    /// The description editing began from.
    base: String,
    /// Whose draft it is, fixed when editing began: an account switched
    /// meanwhile does not split it.
    owner: Option<DraftOwner>,
    pub state: SaveState,
    /// Redraws the detail as it is typed in.
    _typing: Subscription,
}

impl std::fmt::Debug for BodyEdit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BodyEdit")
            .field("input", &self.input.entity_id())
            .field("state", &self.state)
            .finish()
    }
}

impl PrDetailState {
    pub(in crate::workspace) fn conversation(&self) -> Option<&Load<Conversation>> {
        self.conversation.as_ref().map(ReadSlot::load)
    }

    /// The description as last read, for a test.
    #[cfg(test)]
    pub(in crate::workspace) fn idle_read_body(&self) -> Option<String> {
        self.pr.last_known().map(|pr| pr.body.clone())
    }

    pub(in crate::workspace) fn body_edit(&self) -> Option<&BodyEdit> {
        self.body_edit.as_ref()
    }

    /// Read the conversation, the first time or after a failed read.
    pub(super) fn conversation_wanted(&self) -> bool {
        matches!(self.conversation(), None | Some(Load::Failed(_)))
    }

    /// Read it again, keeping what is shown until the answer.
    pub(super) fn reread_conversation(&mut self) -> ReadTicket {
        let ticket = self.tickets.issue();
        ReadSlot::begin(&mut self.conversation, ticket);
        ticket
    }

    pub(super) fn begin_conversation_read(&mut self) -> ReadTicket {
        let ticket = self.tickets.issue();
        self.conversation = Some(ReadSlot::reading(ticket));
        ticket
    }
}

impl Workspace {
    pub(super) fn send_pr_conversation_read(
        &mut self,
        id: PrDetailId,
        ticket: ReadTicket,
        cx: &mut Context<Self>,
    ) {
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        let (repo, number) = (detail.item.repo.clone(), detail.item.pr.number);
        let gh = GhHost::global(cx);
        spawn_bg_work_and_mutate(
            cx,
            move || gh.pr_conversation(&repo, number),
            move |ws, result, cx| ws.land_pr_conversation(id, ticket, result, cx),
        )
        .detach();
    }

    pub(in crate::workspace) fn land_pr_conversation(
        &mut self,
        id: PrDetailId,
        ticket: ReadTicket,
        result: Result<Conversation, GhError>,
        cx: &mut Context<Self>,
    ) {
        let Some(slot) = self.pr_detail_mut(id).and_then(|d| d.conversation.as_mut()) else {
            return;
        };
        let (answer, error) = match result {
            Ok(read) => (Ok(read), None),
            Err(e) => (Err(e.to_string()), Some(e)),
        };
        if !slot.land(ticket, answer) {
            return;
        }
        if let Some(e) = error {
            self.report_pr_read(&e, "github.pr_conversation.read", cx);
        }
        self.follow_pr_comment(id, cx);
        cx.notify();
    }

    /// Edit the description, starting from what was read.
    pub(in crate::workspace) fn edit_pr_body(
        &mut self,
        id: PrDetailId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(body) = self.pr_detail_mut(id).and_then(|detail| {
            let loaded = detail.pr.last_known()?;
            (loaded.can_edit() && detail.body_edit.is_none()).then(|| loaded.body.clone())
        }) else {
            return;
        };
        // A draft kept from before continues where it was left, against
        // the description it began from.
        let owner = self.pr_draft_owner(id, cx);
        let draft = owner
            .as_ref()
            .and_then(|owner| DraftStore::of(owner, cx))
            .and_then(|drafts| drafts.description);
        let (base, text) = match draft {
            Some(draft) => (draft.base, draft.text),
            None => (body.clone(), body),
        };
        let input = crate::ui::make_markdown_prose_state(
            &text,
            "",
            crate::ui::theme::PR_BODY_EDIT_ROWS,
            window,
            cx,
        );
        let typing = cx.subscribe(&input, move |ws, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                ws.pr_body_typed(id, cx);
            }
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        if let Some(detail) = self.pr_detail_mut(id) {
            detail.body_edit = Some(BodyEdit {
                input,
                base,
                owner,
                state: SaveState::Editing,
                _typing: typing,
            });
        }
        cx.notify();
    }

    /// Stop editing, dropping the edit; not while it saves.
    pub(in crate::workspace) fn cancel_pr_body_edit(
        &mut self,
        id: PrDetailId,
        cx: &mut Context<Self>,
    ) {
        if let Some(detail) = self.pr_detail_mut(id)
            && detail
                .body_edit
                .as_ref()
                .is_some_and(|edit| edit.state != SaveState::Saving)
        {
            detail.body_edit = None;
            self.drop_pr_description_draft(id, cx);
            cx.notify();
        }
    }

    /// Typing in the editor: a failure or conflict line no longer
    /// describes this text.
    fn pr_body_typed(&mut self, id: PrDetailId, cx: &mut Context<Self>) {
        let Some(edit) = self.pr_detail_mut(id).and_then(|d| d.body_edit.as_mut()) else {
            return;
        };
        if matches!(edit.state, SaveState::Failed(_)) {
            edit.state = SaveState::Editing;
        }
        // Kept as typed, so leaving or quitting loses none of it.
        let draft = DescriptionDraft {
            base: edit.base.clone(),
            text: edit.input.read(cx).value().to_string(),
        };
        if let Some(owner) = edit.owner.clone() {
            DraftStore::set_description(&owner, Some(draft), cx);
        }
        cx.notify();
    }

    fn drop_pr_description_draft(&mut self, id: PrDetailId, cx: &mut Context<Self>) {
        if let Some(owner) = self.pr_draft_owner(id, cx) {
            DraftStore::set_description(&owner, None, cx);
        }
    }

    /// Save the edited description to GitHub, unless it changed there
    /// since editing began.
    pub(in crate::workspace) fn save_pr_body(&mut self, id: PrDetailId, cx: &mut Context<Self>) {
        self.save_pr_body_over(id, false, cx);
    }

    /// Save it even though it changed on GitHub, replacing that change.
    pub(in crate::workspace) fn overwrite_pr_body(
        &mut self,
        id: PrDetailId,
        cx: &mut Context<Self>,
    ) {
        self.save_pr_body_over(id, true, cx);
    }

    fn save_pr_body_over(&mut self, id: PrDetailId, overwrite: bool, cx: &mut Context<Self>) {
        let Some(body) = self
            .pr_detail_mut(id)
            .and_then(|detail| detail.body_edit.as_ref())
            .filter(|edit| edit.state != SaveState::Saving)
            .map(|edit| edit.input.read(cx).value().to_string())
        else {
            return;
        };
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        let on_github = detail.pr.last_known().map(|pr| pr.body.clone());
        let Some(edit) = detail.body_edit.as_mut() else {
            return;
        };
        // A refresh brought a description other than the one editing
        // began from: someone else changed it.
        let moved = on_github
            .as_deref()
            .is_some_and(|now| !same_text(now, &edit.base));
        if moved && !overwrite {
            edit.state = SaveState::Conflict;
            cx.notify();
            return;
        }
        if same_text(&body, on_github.as_deref().unwrap_or(&edit.base)) {
            // Nothing to change on GitHub.
            detail.body_edit = None;
            self.drop_pr_description_draft(id, cx);
            cx.notify();
            return;
        }
        edit.state = SaveState::Saving;
        let base = edit.base.clone();
        let (repo, number) = (detail.item.repo.clone(), detail.item.pr.number);
        let gh = GhHost::global(cx);
        let bg = repo.clone();
        spawn_bg_work_and_mutate(
            cx,
            move || {
                // What was last read can be old, or a refresh out: GitHub's
                // description is read again right before it is replaced.
                // `gh pr edit` takes no expected body to compare for us.
                if !overwrite && !same_text(&gh.pr_detail(&bg, number)?.body, &base) {
                    return Ok(BodySave::Moved);
                }
                gh.edit_pr_body(&bg, number, &body)?;
                Ok(BodySave::Saved)
            },
            move |ws, result, cx| ws.land_pr_body(id, repo, number, result, cx),
        )
        .detach();
        cx.notify();
    }

    /// Saved: the editor closes and the pull request is read again, so
    /// what shows is what GitHub holds. Changed there meanwhile: nothing was
    /// sent, the text stays, and the detail reads the change to show it.
    /// Failed: the text stays. Reported even if the detail is gone.
    fn land_pr_body(
        &mut self,
        id: PrDetailId,
        repo: daruda_github::RepoId,
        number: u64,
        result: Result<BodySave, GhError>,
        cx: &mut Context<Self>,
    ) {
        let repo_for_draft = repo.clone();
        PrPoller::nudge(repo, cx);
        let saving = self
            .pr_detail_mut(id)
            .and_then(|detail| detail.body_edit.as_mut())
            .filter(|edit| edit.state == SaveState::Saving);
        match result {
            Ok(BodySave::Moved) => {
                if let Some(edit) = saving {
                    edit.state = SaveState::Conflict;
                }
                self.refresh_pr_detail(id, cx);
            }
            Ok(BodySave::Saved) => {
                if saving.is_some()
                    && let Some(detail) = self.pr_detail_mut(id)
                {
                    detail.body_edit = None;
                }
                // Published: the draft is done with, even if the detail is
                // gone.
                if let Some(owner) = DraftStore::owner(&repo_for_draft, number, cx) {
                    DraftStore::set_description(&owner, None, cx);
                }
                self.refresh_pr_detail(id, cx);
            }
            Err(e) => {
                if let Some(edit) = saving {
                    edit.state = SaveState::Failed(e.to_string());
                }
                let report = ErrorReport::new(s::body_save_failed())
                    .severity(ErrorSeverity::Error)
                    .from_error(&e)
                    .at(file!(), line!())
                    .dedup("github.pr_body.save")
                    .build();
                self.report_error(report, cx);
            }
        }
        cx.notify();
    }
}

impl Workspace {
    /// Whose drafts the detail's pull request holds.
    pub(super) fn pr_draft_owner(
        &mut self,
        id: PrDetailId,
        cx: &Context<Self>,
    ) -> Option<DraftOwner> {
        let detail = self.pr_detail_mut(id)?;
        DraftStore::owner(&detail.item.repo, detail.item.pr.number, cx)
    }
}

/// The same text, whichever line endings it was written with.
pub(super) fn same_text(a: &str, b: &str) -> bool {
    a.replace("\r\n", "\n") == b.replace("\r\n", "\n")
}

#[cfg(test)]
mod tests;
