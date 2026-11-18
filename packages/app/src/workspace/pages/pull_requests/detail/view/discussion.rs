//! The Description tab's conversation and edit, review comments in the
//! Files tab, and a handoff to an agent.

use super::*;

/// The Description tab beyond the description: editing it, and the
/// conversation under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct DiscussionView {
    /// GitHub lets the viewer edit the description.
    pub can_edit: bool,
    /// An edit of it is kept in draft.
    pub draft: bool,
    /// Whether what is typed is kept across restarts.
    pub keeping: crate::github::DraftKeeping,
    /// The description being edited, and how saving it stands.
    pub edit: Option<(
        gpui::Entity<crate::ui::TextareaState>,
        super::super::description::SaveState,
    )>,
    pub conversation: ConversationView,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) enum ConversationView {
    Loading,
    Failed(String),
    Listed {
        comments: Vec<CommentView>,
        truncated: bool,
    },
}

/// Comments on lines kept for a review not yet submitted, and the one being
/// written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct ReviewCommentsView {
    pub pending: Vec<PendingCommentView>,
    pub composer: Option<ComposerView>,
    /// Whether what is typed is kept across restarts.
    pub keeping: crate::github::DraftKeeping,
    /// The review being written, once begun.
    pub review: Option<ReviewView>,
    /// A submission is in flight or its outcome unknown: pending comments
    /// are not changed meanwhile.
    pub locked: bool,
}

/// The review being written: text, verdict and how submitting stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct ReviewView {
    pub body: gpui::Entity<crate::ui::TextareaState>,
    pub event: daruda_github::ReviewEvent,
    pub state: super::super::review_submit::SubmitState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct PendingCommentView {
    pub id: u64,
    /// `path:12-15 (new)`.
    pub reference: String,
    /// The first line of its text.
    pub excerpt: String,
    pub suggests: bool,
    /// Written on another head than the one listed: its lines may now be
    /// other code.
    pub outdated: bool,
    /// Its lines are not all in the diff within one hunk, which GitHub
    /// refuses to anchor a review comment to.
    pub unanchored: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct ComposerView {
    pub reference: String,
    pub outdated: bool,
    pub unanchored: bool,
    pub body: gpui::Entity<crate::ui::TextareaState>,
    pub suggestion: Option<gpui::Entity<crate::ui::TextareaState>>,
    /// Replacement text applies to the head's lines only.
    pub can_suggest: bool,
}

/// A handoff to an agent: what, where, and the prompt prepared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct HandoffView {
    pub payload: super::super::handoff::Payload,
    /// The worktree's name.
    pub lane: String,
    /// Why nothing can be inserted as prepared, if so.
    pub blocked: Option<HandoffBlock>,
    /// Chat, title and tab number.
    pub chats: Vec<(
        crate::workspace::main_area::pane_tree::PaneId,
        gpui::SharedString,
        usize,
    )>,
    pub chosen: Option<crate::workspace::main_area::pane_tree::PaneId>,
    pub prompt: super::super::handoff::Prepared,
}

/// Why a prepared handoff cannot go in, the first that applies. The
/// handlers check the worktree and the selection again as they run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum HandoffBlock {
    /// The pull request's worktree is no longer this one.
    Moved,
    /// The lines quoted are no longer the ones selected.
    SelectionChanged,
    /// The chat chosen is no longer a writable chat there: pick another.
    ChatGone,
}

/// What holds a handoff back, one at a time: the first in this order is
/// the one to fix first, since fixing it can settle the others.
pub(super) fn handoff_block(
    moved: bool,
    selection_changed: bool,
    chat_gone: bool,
) -> Option<HandoffBlock> {
    if moved {
        Some(HandoffBlock::Moved)
    } else if selection_changed {
        Some(HandoffBlock::SelectionChanged)
    } else if chat_gone {
        Some(HandoffBlock::ChatGone)
    } else {
        None
    }
}

pub(super) fn handoff_view(
    handoff: &super::super::handoff::Handoff,
    state: &PrDetailState,
    inputs: &DetailInputs<'_>,
) -> HandoffView {
    let chats = &inputs.handoff_chats;
    let listed = |pane| chats.iter().any(|(held, _, _)| *held == pane);
    // Nothing chosen yet: the first listed, as Insert would take. A chosen
    // chat gone is never swapped for another.
    let chosen = match handoff.chat {
        Some(chat) => listed(chat).then_some(chat),
        None => chats.first().map(|(pane, _, _)| *pane),
    };
    let selection_changed = match &handoff.payload {
        super::super::handoff::Payload::SelectedCode(range) => !state
            .files()
            .is_some_and(|files| files.selection_current(range)),
        _ => false,
    };
    let blocked = handoff_block(
        inputs.lane.map(|lane| lane.lane) != Some(handoff.lane),
        selection_changed,
        // With none left, a new chat is offered instead of another choice.
        !chats.is_empty() && handoff.chat.is_some_and(|chat| !listed(chat)),
    );
    HandoffView {
        payload: handoff.payload.clone(),
        lane: handoff.lane_name.clone(),
        blocked,
        chats: chats.clone(),
        chosen,
        prompt: handoff.prompt.clone(),
    }
}

pub(super) fn review_comments(
    state: &PrDetailState,
    drafts: Option<&daruda_store::review_drafts::PrDrafts>,
    keeping: crate::github::DraftKeeping,
) -> ReviewCommentsView {
    let files = state.files();
    let listed_at = files.and_then(|files| files.revision());
    let unanchored = |range: &super::super::diff_layout::LineRange| {
        files.and_then(|f| f.anchors(range)) == Some(false)
    };
    let range =
        |path: &str, side, start, end, revision: &str| super::super::diff_layout::LineRange {
            path: path.to_owned(),
            side,
            start,
            end,
            revision: revision.to_owned(),
        };
    ReviewCommentsView {
        pending: drafts
            .map(|drafts| drafts.comments.as_slice())
            .unwrap_or_default()
            .iter()
            .map(|draft| {
                let lines = range(
                    &draft.path,
                    super::super::line_comments::view_side(draft.side),
                    draft.start,
                    draft.end,
                    &draft.revision,
                );
                (draft, lines)
            })
            .map(|(draft, lines)| PendingCommentView {
                id: draft.id,
                reference: lines.place(),
                unanchored: unanchored(&lines),
                excerpt: excerpt(&draft.body),
                suggests: draft.suggestion.is_some(),
                outdated: listed_at.is_some_and(|head| head != draft.revision),
            })
            .collect(),
        composer: state.composer().map(|composer| ComposerView {
            reference: composer.range.place(),
            outdated: listed_at.is_some_and(|head| head != composer.range.revision),
            unanchored: unanchored(&composer.range),
            body: composer.body.clone(),
            suggestion: composer.suggestion.clone(),
            can_suggest: composer.range.side == super::super::diff_layout::Side::New,
        }),
        keeping,
        locked: state
            .review()
            .is_some_and(|review| !review.state.editable()),
        review: state.review().map(|review| ReviewView {
            body: review.body.clone(),
            event: review.event,
            state: review.state.clone(),
        }),
    }
}
