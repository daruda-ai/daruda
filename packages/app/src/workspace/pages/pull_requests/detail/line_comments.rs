//! Comments on lines of the diff for a review not yet submitted: written
//! from the Files tab's selection, kept as drafts as they are typed, edited
//! and discarded from the list of pending comments. Nothing here posts;
//! submitting a review does.

use daruda_store::review_drafts::{DraftOwner, DraftSide, LineCommentDraft};
use gpui::{Context, Entity, Subscription, Window};

use super::PrDetailId;
use super::diff_layout::{LineRange, Side};
use crate::github::DraftStore;
use crate::surface::strings::pull_requests as s;
use crate::ui::{InputEvent, TextareaState};
use crate::workspace::Workspace;

/// A line comment being written.
pub(in crate::workspace) struct LineComposer {
    pub range: LineRange,
    pub body: Entity<TextareaState>,
    /// Replacement text for the lines; new-side lines only.
    pub suggestion: Option<Entity<TextareaState>>,
    /// The draft it is kept as; `None` until anything is typed.
    id: Option<u64>,
    /// Whose draft it is, fixed when it opened.
    owner: Option<DraftOwner>,
    _typing: Vec<Subscription>,
}

impl LineComposer {
    pub(super) fn id(&self) -> Option<u64> {
        self.id
    }
}

impl std::fmt::Debug for LineComposer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LineComposer")
            .field("range", &self.range)
            .field("id", &self.id)
            .finish()
    }
}

/// The draft's side, and back.
pub(in crate::workspace) fn draft_side(side: Side) -> DraftSide {
    match side {
        Side::Old => DraftSide::Left,
        Side::New => DraftSide::Right,
    }
}

pub(in crate::workspace) fn view_side(side: DraftSide) -> Side {
    match side {
        DraftSide::Left => Side::Old,
        DraftSide::Right => Side::New,
    }
}

impl Workspace {
    /// Write a comment on the lines selected in the Files tab.
    pub(in crate::workspace) fn comment_pr_lines(
        &mut self,
        id: PrDetailId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(range) = self
            .pr_detail_mut(id)
            .and_then(|detail| detail.files())
            .and_then(|files| files.selection().cloned())
        else {
            return;
        };
        self.open_line_composer(id, ComposerSeed::new(range), window, cx);
    }

    /// Edit the pending comment `comment` again, showing its lines. A
    /// composer already open gives way: its text is kept as typed.
    pub(in crate::workspace) fn edit_pr_line_comment(
        &mut self,
        id: PrDetailId,
        comment: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .pr_detail(id)
            .is_none_or(|detail| detail.review_comments_locked())
        {
            return;
        }
        let Some(draft) = self
            .pr_draft_owner(id, cx)
            .and_then(|owner| DraftStore::of(&owner, cx))
            .and_then(|drafts| drafts.comments.into_iter().find(|held| held.id == comment))
        else {
            return;
        };
        let range = LineRange {
            path: draft.path,
            side: view_side(draft.side),
            start: draft.start,
            end: draft.end,
            revision: draft.revision,
        };
        if let Some(detail) = self.pr_detail_mut(id) {
            detail.composer = None;
            // Its lines, shown where they are when the listing is at the
            // commit it was written on.
            if let Some(files) = detail.files.as_mut() {
                if files.revision() == Some(range.revision.as_str()) {
                    files.go_to_lines(super::view::ThreadTarget {
                        path: range.path.clone(),
                        side: range.side,
                        start: range.start,
                        end: range.end,
                        revision: range.revision.clone(),
                    });
                } else {
                    files.show_listed(&range.path);
                }
            }
            detail.reveal_file_row();
            detail.scroll.set_offset(gpui::Point::default());
        }
        let seed = ComposerSeed {
            range,
            id: Some(comment),
            body: draft.body,
            suggestion: draft.suggestion,
        };
        self.open_line_composer(id, seed, window, cx);
    }

    fn open_line_composer(
        &mut self,
        id: PrDetailId,
        seed: ComposerSeed,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .pr_detail_mut(id)
            .is_none_or(|detail| detail.composer.is_some() || detail.review_comments_locked())
        {
            return;
        }
        let ComposerSeed {
            range,
            id: comment,
            body,
            suggestion,
        } = seed;
        let owner = self.pr_draft_owner(id, cx);
        let rows = crate::ui::theme::PR_REPLY_ROWS;
        let body =
            crate::ui::make_markdown_prose_state(&body, s::comment_placeholder(), rows, window, cx);
        let suggestion = suggestion
            .map(|text| crate::ui::make_markdown_prose_state(&text, "", rows, window, cx));
        let mut typing = vec![self.composer_typing(id, &body, cx)];
        typing.extend(
            suggestion
                .as_ref()
                .map(|state| self.composer_typing(id, state, cx)),
        );
        body.update(cx, |body, cx| body.focus(window, cx));
        if let Some(detail) = self.pr_detail_mut(id) {
            detail.composer = Some(LineComposer {
                range,
                body,
                suggestion,
                id: comment,
                owner,
                _typing: typing,
            });
        }
        cx.notify();
    }

    fn composer_typing(
        &mut self,
        id: PrDetailId,
        state: &Entity<TextareaState>,
        cx: &mut Context<Self>,
    ) -> Subscription {
        cx.subscribe(state, move |ws, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                ws.pr_comment_typed(id, cx);
            }
        })
    }

    /// Add replacement text for the comment's lines, starting from them
    /// when the diff holds every one within a hunk (empty otherwise, never
    /// other code); or drop it.
    pub(in crate::workspace) fn toggle_pr_suggestion(
        &mut self,
        id: PrDetailId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        let Some(composer) = detail.composer.as_mut() else {
            return;
        };
        let lines = detail
            .files
            .as_ref()
            .and_then(|files| files.lines_text(&composer.range));
        if composer.suggestion.take().is_some() || composer.range.side != Side::New {
            self.pr_comment_typed(id, cx);
            cx.notify();
            return;
        }
        let state = crate::ui::make_markdown_prose_state(
            &lines.unwrap_or_default(),
            "",
            crate::ui::theme::PR_REPLY_ROWS,
            window,
            cx,
        );
        let typing = self.composer_typing(id, &state, cx);
        if let Some(composer) = self.pr_detail_mut(id).and_then(|d| d.composer.as_mut()) {
            composer.suggestion = Some(state);
            composer._typing.push(typing);
        }
        self.pr_comment_typed(id, cx);
        cx.notify();
    }

    /// Typed: kept as the comment's draft at once, so nothing typed is
    /// lost by leaving or quitting.
    fn pr_comment_typed(&mut self, id: PrDetailId, cx: &mut Context<Self>) {
        let Some(composer) = self.pr_detail_mut(id).and_then(|d| d.composer.as_mut()) else {
            return;
        };
        let body = composer.body.read(cx).value().to_string();
        let suggestion = composer
            .suggestion
            .as_ref()
            .map(|state| state.read(cx).value().to_string());
        let Some(owner) = composer.owner.clone() else {
            cx.notify();
            return;
        };
        if composer.id.is_none() && body.trim().is_empty() && suggestion.is_none() {
            return;
        }
        let range = &composer.range;
        let draft = LineCommentDraft {
            id: 0,
            path: range.path.clone(),
            side: draft_side(range.side),
            start: range.start,
            end: range.end,
            revision: range.revision.clone(),
            body,
            suggestion,
        };
        let kept = DraftStore::put_comment(&owner, composer.id, draft, cx);
        if let Some(composer) = self.pr_detail_mut(id).and_then(|d| d.composer.as_mut()) {
            composer.id = Some(kept);
        }
        cx.notify();
    }

    /// Move the open draft only after a fresh selection was explicitly accepted.
    pub(in crate::workspace) fn reanchor_pr_comment(
        &mut self,
        id: PrDetailId,
        cx: &mut Context<Self>,
    ) {
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        let Some(range) = detail.comment_replacement_range().cloned() else {
            return;
        };
        let Some(composer) = detail.composer.as_mut() else {
            return;
        };
        composer.range = range;
        self.pr_comment_typed(id, cx);
        self.close_pr_line_comment(id, cx);
    }

    /// Done writing: the comment stays pending in the review, unless it
    /// was left with nothing in it.
    pub(in crate::workspace) fn close_pr_line_comment(
        &mut self,
        id: PrDetailId,
        cx: &mut Context<Self>,
    ) {
        let Some(composer) = self.pr_detail_mut(id).and_then(|d| d.composer.take()) else {
            return;
        };
        if let Some(detail) = self.pr_detail_mut(id)
            && detail.review.is_some()
        {
            detail.scroll.set_offset(gpui::Point::default());
        }
        let empty = composer.body.read(cx).value().trim().is_empty()
            && composer
                .suggestion
                .as_ref()
                .is_none_or(|state| state.read(cx).value().trim().is_empty());
        if empty && let (Some(kept), Some(owner)) = (composer.id, composer.owner) {
            DraftStore::remove_comment(&owner, kept, cx);
        }
        cx.notify();
    }

    /// Drop the pending comment `comment`, and its composer if open.
    pub(in crate::workspace) fn discard_pr_line_comment(
        &mut self,
        id: PrDetailId,
        comment: u64,
        cx: &mut Context<Self>,
    ) {
        if self
            .pr_detail(id)
            .is_none_or(|detail| detail.review_comments_locked())
        {
            return;
        }
        if let Some(owner) = self.pr_draft_owner(id, cx) {
            DraftStore::remove_comment(&owner, comment, cx);
        }
        if let Some(detail) = self.pr_detail_mut(id)
            && detail
                .composer
                .as_ref()
                .is_some_and(|composer| composer.id == Some(comment))
        {
            detail.composer = None;
        }
        cx.notify();
    }

    /// Drop the comment being written: the composer and anything kept of it.
    pub(in crate::workspace) fn discard_pr_composer(
        &mut self,
        id: PrDetailId,
        cx: &mut Context<Self>,
    ) {
        let Some(composer) = self.pr_detail_mut(id).and_then(|d| d.composer.take()) else {
            return;
        };
        if let (Some(kept), Some(owner)) = (composer.id, composer.owner) {
            DraftStore::remove_comment(&owner, kept, cx);
        }
        cx.notify();
    }
}

impl super::PrDetailState {
    pub(super) fn review_comments_locked(&self) -> bool {
        self.review().is_some_and(|review| !review.state.editable())
    }

    pub(super) fn comment_replacement_range(&self) -> Option<&LineRange> {
        let composer = self.composer.as_ref()?;
        let files = self.files()?;
        let head = &self.pr.last_known()?.head_sha;
        let selection = files.selection()?;
        if selection.revision != *head
            || selection == &composer.range
            || (composer.suggestion.is_some() && selection.side == Side::Old)
            || (composer.range.revision == *head && files.anchors(&composer.range) != Some(false))
            || files.anchors(selection) != Some(true)
        {
            return None;
        }
        Some(selection)
    }
}

/// What a composer opens with.
struct ComposerSeed {
    range: LineRange,
    id: Option<u64>,
    body: String,
    suggestion: Option<String>,
}

impl ComposerSeed {
    fn new(range: LineRange) -> Self {
        Self {
            range,
            id: None,
            body: String::new(),
            suggestion: None,
        }
    }
}

#[cfg(test)]
mod tests;
