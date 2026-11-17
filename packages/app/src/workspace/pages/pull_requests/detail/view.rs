//! What the detail draws, built from plain values so every case is testable
//! without a window: the header, each tab's content, the summary column, and
//! whether Send to agent can go anywhere.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use daruda_github::{
    ActionsJob, BlockReason, CheckEntry, CheckStatus, MergeBlock, MergeMethod, Mergeability,
    PrDetail, PrListItem, PrState, QueueState, ReviewThread, ReviewThreads, ReviewerState,
};

use super::super::actions::{self, ActionState, PrAction};
use super::super::list::{self, LaneFacts, LaneSlot, PrKey, RowLane};
use super::{Load, PrDetailId, PrDetailState, PrTab};
use crate::surface::strings::{git as git_s, notification, pull_requests as s};

/// How a value reads: the colour it is drawn in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum Mood {
    Good,
    Warn,
    Bad,
    Quiet,
}

/// Whether Send to agent has somewhere to go and something to send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum SendSlot {
    /// No worktree works on the pull request, so no agent can take it.
    NoLane,
    /// Nothing failing, or no open thread.
    Nothing,
    Sending,
    Ready,
}

impl SendSlot {
    pub(in crate::workspace) fn enabled(self) -> bool {
        self == Self::Ready
    }
}

/// Whether an action on the pull request can be pressed, and why not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) enum ActionSlot {
    Ready,
    /// This action is running.
    Running,
    /// Another action on the same pull request is running.
    Waiting,
    /// Said in its tooltip.
    Blocked(String),
}

impl ActionSlot {
    /// `mine` running, another running, or neither and `blocked` or not.
    fn of(state: &ActionState, mine: PrAction, blocked: Option<String>) -> Self {
        match state.running() {
            None => blocked.map_or(Self::Ready, Self::Blocked),
            Some(running) if running == mine => Self::Running,
            Some(_) => Self::Waiting,
        }
    }

    pub(in crate::workspace) fn enabled(&self) -> bool {
        *self == Self::Ready
    }

    pub(in crate::workspace) fn reason(&self) -> Option<String> {
        match self {
            Self::Blocked(reason) => Some(reason.clone()),
            Self::Ready | Self::Running | Self::Waiting => None,
        }
    }
}

/// The merge button and its menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct MergeView {
    /// The preferred method's label, or a plain one when none is allowed.
    pub label: String,
    /// The allowed methods, preferred first, and their labels.
    pub methods: Vec<(MergeMethod, String)>,
    /// The methods auto-merge can be turned on with, and their labels.
    pub auto: Vec<(MergeMethod, String)>,
    pub slot: ActionSlot,
}

/// What the header offers to do to the pull request, by its state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) enum HeaderControls {
    /// Not read yet, or merged: nothing.
    None,
    /// Open or a draft.
    Active(HeaderActions),
    /// Closed unmerged.
    Reopen(ActionSlot),
}

#[cfg(test)]
impl HeaderControls {
    pub(in crate::workspace) fn actions(&self) -> Option<&HeaderActions> {
        match self {
            Self::Active(actions) => Some(actions),
            Self::None | Self::Reopen(_) => None,
        }
    }

    pub(in crate::workspace) fn reopen(&self) -> Option<&ActionSlot> {
        match self {
            Self::Reopen(slot) => Some(slot),
            Self::None | Self::Active(_) => None,
        }
    }
}

/// The header's actions on an open or draft pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct HeaderActions {
    pub merge: MergeView,
    pub close: ActionSlot,
    /// Only on a draft.
    pub ready: Option<ActionSlot>,
    /// Only while auto-merge is on and the viewer may turn it off.
    pub auto_merge_off: Option<ActionSlot>,
}

/// What the summary column says under the linked worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum LaneNote {
    /// On the pull request's own branch: a push updates it.
    UpdatesPr,
    /// A fork's review worktree: its pushes go nowhere near it.
    ReviewOnly,
}

/// A failed Actions check's log, which it can open under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) enum LogSlot {
    /// Not a failed Actions job: nothing to open.
    None,
    Closed(ActionsJob),
    Open(ActionsJob, Load<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct CheckRowView {
    pub status: CheckStatus,
    pub name: String,
    /// `workflow · event`, for an Actions check.
    pub context: Option<String>,
    pub duration: Option<String>,
    pub log: LogSlot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct ThreadRowView {
    /// GitHub's id for the thread, which expanding it names.
    pub id: String,
    /// `path:L12–L20`.
    pub location: String,
    pub author: String,
    /// The first line of the first comment.
    pub excerpt: String,
    pub replies: Option<String>,
    pub outdated: bool,
    /// The thread's first comment on GitHub.
    pub url: Option<String>,
    /// Where it sits in the current diff; `None` for an outdated thread,
    /// whose lines are where the code was, or a file-level one.
    pub target: Option<ThreadTarget>,
    /// Every comment in full, once the thread is expanded.
    pub comments: Option<Vec<CommentView>>,
    /// Resolved from this detail since the threads were read; it can be
    /// unresolved again until they are read anew.
    pub resolved_here: bool,
    /// The reply being written, if a box is open.
    pub reply: Option<gpui::Entity<crate::ui::TextareaState>>,
    /// The thread's last action, running or failed.
    pub op: Option<super::thread_actions::ThreadOp>,
    /// A failed reply may have posted: the threads are being read again
    /// before it can be posted again.
    pub checking: bool,
    /// What the viewer may do to it.
    pub may: daruda_github::ThreadPermissions,
    /// A reply to it is kept in draft.
    pub draft: bool,
}

/// Lines of the current diff a thread is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct ThreadTarget {
    pub path: String,
    pub side: super::diff_layout::Side,
    pub start: u32,
    pub end: u32,
}

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
        super::description::SaveState,
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
    pub state: super::review_submit::SubmitState,
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

/// One comment of an expanded thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct CommentView {
    /// GitHub's id, which keys its drawn Markdown.
    pub id: String,
    pub author: String,
    /// Markdown, drawn as such.
    pub body: String,
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) enum ThreadsView {
    Loading,
    Failed(String),
    Listed {
        rows: Vec<ThreadRowView>,
        truncated: bool,
        /// Whether replies typed now are kept across restarts.
        keeping: crate::github::DraftKeeping,
        /// Reply drafts kept for threads no longer listed: thread id and
        /// text, shown so they can be copied before being discarded.
        orphan_drafts: Vec<(String, String)>,
    },
}

/// One `label: value` line of the summary column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct Fact {
    pub label: String,
    pub value: String,
    pub mood: Mood,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct SideView {
    pub merge: Vec<Fact>,
    /// Each reviewer and where they stand, then the open threads.
    pub reviews: Vec<Fact>,
    pub lane: Option<RowLane>,
    /// Said under the linked worktree: whether its pushes reach the PR.
    pub lane_note: Option<LaneNote>,
    /// Who is asked to review can be changed: GitHub lets the viewer.
    pub requests: Option<RequestsView>,
}

/// Changing who is asked to review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct RequestsView {
    /// Asked and not reviewed yet: these can be unasked.
    pub removable: Vec<String>,
    pub input: Option<gpui::Entity<crate::ui::InputState>>,
    pub op: Option<super::reviewers::ReviewerOp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct ReadyView {
    pub checks_summary: String,
    pub checks: Vec<CheckRowView>,
    pub checks_send: SendSlot,
    /// Only when a failed check is an Actions job, which can be rerun.
    pub rerun: Option<ActionSlot>,
    pub threads: ThreadsView,
    pub threads_send: SendSlot,
    /// Send to agent for the lines selected in the Files tab.
    pub code_send: SendSlot,
    /// Only while the Files tab is shown: the listing can be long.
    pub files: Option<super::files::FilesView>,
    /// Only while the Files tab is shown: comments pending for a review.
    pub review_comments: Option<ReviewCommentsView>,
    /// `None` when the author wrote none.
    pub description: Option<String>,
    /// Only while the Description tab is shown.
    pub discussion: Option<DiscussionView>,
    pub side: SideView,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) enum DetailBody {
    Loading,
    Failed(String),
    Ready(Box<ReadyView>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct PrDetailView {
    pub id: PrDetailId,
    pub state: String,
    pub state_mood: Mood,
    /// `#480 Adjust sidebar…`.
    pub title: String,
    /// `head → base · author · 2h ago · Changes requested · Behind the base`.
    pub meta: String,
    pub url: Option<String>,
    pub lane: LaneSlot,
    pub header: HeaderControls,
    /// What the last action's failure said, until the next action or reload.
    pub error: Option<String>,
    pub tab: PrTab,
    pub tabs: Vec<(PrTab, String)>,
    pub handoff: Option<HandoffView>,
    pub body: DetailBody,
}

/// What the detail reads beside its own state.
pub(in crate::workspace) struct DetailInputs<'a> {
    /// The worktree working on the pull request, if any.
    pub lane: Option<&'a LaneFacts>,
    /// Whether a send to that worktree's chat is under way.
    pub sending: bool,
    /// Pull requests a worktree is being made for.
    pub creating: &'a BTreeSet<PrKey>,
    /// Which files were marked viewed.
    pub progress: &'a super::progress::ReviewProgress,
    /// Unsent review text kept for this pull request.
    pub drafts: Option<daruda_store::review_drafts::PrDrafts>,
    /// Whether what is typed now is kept across restarts.
    pub keeping: crate::github::DraftKeeping,
    /// The writable agent chats of the worktree a handoff goes to, the one
    /// it would pick first; staged only while a handoff is open.
    pub handoff_chats: Vec<(
        crate::workspace::main_area::pane_tree::PaneId,
        gpui::SharedString,
        usize,
    )>,
    pub now: DateTime<Utc>,
}

/// A handoff to an agent: what, where, and the prompt prepared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct HandoffView {
    pub payload: super::handoff::Payload,
    /// The worktree's name.
    pub lane: String,
    /// The pull request's worktree is no longer this one: nothing can be
    /// inserted.
    pub moved: bool,
    /// The chat chosen is no longer a writable chat there: pick another.
    pub chat_gone: bool,
    /// The lines quoted are no longer the ones selected: nothing can be
    /// inserted.
    pub selection_changed: bool,
    /// Chat, title and tab number.
    pub chats: Vec<(
        crate::workspace::main_area::pane_tree::PaneId,
        gpui::SharedString,
        usize,
    )>,
    pub chosen: Option<crate::workspace::main_area::pane_tree::PaneId>,
    pub prompt: super::handoff::Prepared,
}

fn handoff_view(
    handoff: &super::handoff::Handoff,
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
    HandoffView {
        payload: handoff.payload.clone(),
        lane: handoff.lane_name.clone(),
        selection_changed: match &handoff.payload {
            super::handoff::Payload::SelectedCode(range) => !state
                .files()
                .is_some_and(|files| files.selection_current(range)),
            _ => false,
        },
        moved: inputs.lane.map(|lane| lane.lane) != Some(handoff.lane),
        // With none left, a new chat is offered instead of another choice.
        chat_gone: !chats.is_empty() && handoff.chat.is_some_and(|chat| !listed(chat)),
        chats: chats.clone(),
        chosen,
        prompt: handoff.prompt.clone(),
    }
}

/// The detail for `state`.
pub(in crate::workspace) fn view(state: &PrDetailState, inputs: &DetailInputs<'_>) -> PrDetailView {
    let item = &state.item;
    let loaded = match state.load() {
        Load::Ready(loaded) => Some(loaded),
        Load::Loading | Load::Failed(_) => None,
    };
    let pr = loaded.map_or(&item.pr, |loaded| &loaded.summary);
    let row_lane = inputs.lane.map(|lane| RowLane {
        lane: lane.lane,
        name: lane.name.clone(),
    });
    let state_shown = if loaded.is_some_and(PrDetail::is_draft) {
        PrState::Draft
    } else {
        pr.state
    };
    let (state_label, state_mood) = state_label(state_shown);
    PrDetailView {
        id: state.id,
        state: state_label,
        state_mood,
        title: format!("{} {}", git_s::pr_number(pr.number), pr.title),
        meta: meta(item, loaded, inputs.now),
        url: pr.web_url().map(str::to_owned),
        lane: LaneSlot::of(item, inputs.lane, inputs.creating),
        header: loaded.map_or(HeaderControls::None, |loaded| {
            header_controls(loaded, &state.action)
        }),
        error: match &state.action {
            ActionState::Failed(line) => Some(line.clone()),
            ActionState::Idle | ActionState::Running(_) => None,
        },
        handoff: state
            .handoff()
            .map(|handoff| handoff_view(handoff, state, inputs)),
        tab: state.tab(),
        tabs: PrTab::ALL
            .into_iter()
            .map(|tab| (tab, tab_label(tab, loaded, state)))
            .collect(),
        body: match state.load() {
            Load::Loading => DetailBody::Loading,
            Load::Failed(reason) => DetailBody::Failed(reason.clone()),
            Load::Ready(loaded) => {
                DetailBody::Ready(Box::new(ready(state, item, loaded, row_lane, inputs)))
            }
        },
    }
}

fn ready(
    state: &PrDetailState,
    item: &PrListItem,
    loaded: &PrDetail,
    lane: Option<RowLane>,
    inputs: &DetailInputs<'_>,
) -> ReadyView {
    let (sending, progress, keeping) = (inputs.sending, inputs.progress, inputs.keeping);
    let drafts = inputs.drafts.as_ref();
    // A handoff being prepared answers its button too.
    let sending = sending
        || state
            .handoff()
            .is_some_and(|handoff| matches!(handoff.prompt, super::handoff::Prepared::Reading(_)));
    let send = |nothing: bool| match (&lane, sending, nothing) {
        (None, _, _) => SendSlot::NoLane,
        (Some(_), true, _) => SendSlot::Sending,
        (Some(_), false, true) => SendSlot::Nothing,
        (Some(_), false, false) => SendSlot::Ready,
    };
    ReadyView {
        checks_summary: checks_summary(loaded),
        checks: loaded
            .checks
            .iter()
            .map(|entry| check_row(entry, state))
            .collect(),
        checks_send: send(loaded.summary.failed.is_empty()),
        rerun: (!loaded.failed_runs().is_empty())
            .then(|| ActionSlot::of(&state.action, PrAction::Rerunning, None)),
        threads: match state.threads() {
            None | Some(Load::Loading) => ThreadsView::Loading,
            Some(Load::Failed(reason)) => ThreadsView::Failed(reason.clone()),
            Some(Load::Ready(threads)) => threads_view(threads, state, drafts, keeping),
        },
        threads_send: send(open_threads(state, loaded) == (0, false)),
        code_send: send(false),
        review_comments: (state.tab() == PrTab::Files)
            .then(|| review_comments(state, drafts, keeping)),
        files: (state.tab() == PrTab::Files).then(|| {
            let marks = progress.marks(&state.key());
            super::files::files_view(state.files(), loaded.changed_files, |file| {
                marks.viewed(&file.path, file.blob.as_deref())
            })
        }),
        discussion: (state.tab() == PrTab::Description).then(|| DiscussionView {
            can_edit: loaded.can_update,
            draft: drafts.is_some_and(|drafts| drafts.description.is_some()),
            keeping,
            edit: state
                .body_edit()
                .map(|edit| (edit.input.clone(), edit.state.clone())),
            conversation: match state.conversation() {
                None | Some(Load::Loading) => ConversationView::Loading,
                Some(Load::Failed(reason)) => ConversationView::Failed(reason.clone()),
                Some(Load::Ready(read)) => ConversationView::Listed {
                    comments: read.comments.iter().map(comment_view).collect(),
                    truncated: read.truncated,
                },
            },
        }),
        description: Some(loaded.body.trim())
            .filter(|body| !body.is_empty())
            .map(str::to_owned),
        side: side(item, loaded, lane, open_threads(state, loaded), state),
    }
}

/// Merge is offered unless it cannot go at all (see [`MergeBlock`]); then
/// its tooltip says why, in the words the summary column uses.
fn header_controls(loaded: &PrDetail, running: &ActionState) -> HeaderControls {
    match loaded.summary.state {
        PrState::Open | PrState::Draft => HeaderControls::Active(header_actions(loaded, running)),
        PrState::Closed => {
            let refused = (!loaded.can_reopen).then(s::reopen_not_allowed);
            HeaderControls::Reopen(ActionSlot::of(running, PrAction::Reopening, refused))
        }
        PrState::Merged => HeaderControls::None,
    }
}

fn header_actions(loaded: &PrDetail, running: &ActionState) -> HeaderActions {
    let draft = loaded.is_draft();
    let methods: Vec<(MergeMethod, String)> = loaded
        .merge_menu()
        .into_iter()
        .map(|method| (method, actions::merge_action_label(method)))
        .collect();
    let blocked = loaded.merge_block().map(|block| match block {
        MergeBlock::NoMethods => s::merge_no_methods(),
        MergeBlock::Queued => s::merge_blocked(s::merge_in_queue()),
        MergeBlock::Draft => s::merge_blocked(s::blocked_draft()),
        MergeBlock::Conflicting => s::merge_blocked(s::merge_conflicting()),
    });
    HeaderActions {
        merge: MergeView {
            label: methods
                .first()
                .map_or_else(s::merge_button, |(_, label)| label.clone()),
            methods,
            auto: loaded
                .auto_merge_menu()
                .into_iter()
                .map(|method| {
                    (
                        method,
                        s::auto_merge_item(actions::merge_action_label(method)),
                    )
                })
                .collect(),
            slot: ActionSlot::of(running, PrAction::Merging, blocked),
        },
        auto_merge_off: loaded
            .can_disable_auto_merge_now()
            .then(|| ActionSlot::of(running, PrAction::DisablingAutoMerge, None)),
        close: ActionSlot::of(
            running,
            PrAction::Closing,
            (!loaded.can_close).then(s::close_not_allowed),
        ),
        ready: draft.then(|| ActionSlot::of(running, PrAction::MarkingReady, None)),
    }
}

fn state_label(state: PrState) -> (String, Mood) {
    match state {
        PrState::Open => (s::state_open(), Mood::Good),
        PrState::Draft => (git_s::pr_state_draft(), Mood::Quiet),
        PrState::Merged => (git_s::pr_state_merged(), Mood::Quiet),
        PrState::Closed => (git_s::pr_state_closed(), Mood::Bad),
    }
}

/// The sub-line: the row's meta, then the review decision and why a merge
/// waits once the detail says.
fn meta(item: &PrListItem, loaded: Option<&PrDetail>, now: DateTime<Utc>) -> String {
    let pr = loaded.map_or(&item.pr, |loaded| &loaded.summary);
    let base = loaded.map_or(&item.base_branch, |loaded| &loaded.base_branch);
    let author = loaded
        .map_or(&item.author, |loaded| &loaded.author)
        .clone()
        .unwrap_or_else(s::author_deleted);
    let mut parts = vec![match list::age_seconds(&pr.updated_at, now) {
        Some(seconds) => s::row_meta(&pr.head_branch, base, author, list::age_label(seconds)),
        None => s::row_meta_undated(&pr.head_branch, base, author),
    }];
    parts.extend(pr.review.map(list::review_label));
    // Why a merge waits says nothing about one closed or merged.
    let waiting = loaded.filter(|loaded| loaded.summary.state.is_active());
    if let Some(Mergeability::Blocked(reason)) = waiting.map(|loaded| loaded.mergeability) {
        parts.push(block_label(reason));
    }
    if waiting.is_some_and(|loaded| loaded.mergeability == Mergeability::Conflicting) {
        parts.push(s::merge_conflicting());
    }
    parts.join(" · ")
}

fn tab_label(tab: PrTab, loaded: Option<&PrDetail>, state: &PrDetailState) -> String {
    let (label, count) = match tab {
        PrTab::Checks => (s::tab_checks(), loaded.map(|l| l.checks.len())),
        PrTab::Files => {
            // GitHub lists at most so many: a capped listing says so.
            // A listing still being read, or capped by GitHub, counts what
            // is in so far and says there is more.
            return match state.files() {
                Some(files) if files.complete() && !files.capped() => {
                    s::tab_count(s::tab_files(), files.len())
                }
                Some(files) if files.len() > 0 => {
                    s::tab_count(s::tab_files(), s::count_capped(files.len()))
                }
                _ => s::tab_files(),
            };
        }
        PrTab::Threads => {
            return match loaded.map(|loaded| open_threads(state, loaded)) {
                Some((count, false)) => s::tab_count(s::tab_threads(), count),
                Some((count, true)) => s::tab_count(s::tab_threads(), s::count_capped(count)),
                None => s::tab_threads(),
            };
        }
        PrTab::Description => return s::tab_description(),
    };
    match count {
        Some(count) => s::tab_count(label, count),
        None => label,
    }
}

fn review_comments(
    state: &PrDetailState,
    drafts: Option<&daruda_store::review_drafts::PrDrafts>,
    keeping: crate::github::DraftKeeping,
) -> ReviewCommentsView {
    let files = state.files();
    let listed_at = files.and_then(|files| files.revision());
    let unanchored =
        |range: &super::diff_layout::LineRange| files.and_then(|f| f.anchors(range)) == Some(false);
    let range = |path: &str, side, start, end, revision: &str| super::diff_layout::LineRange {
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
                    super::line_comments::view_side(draft.side),
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
            can_suggest: composer.range.side == super::diff_layout::Side::New,
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

/// Reviewers asked and not reviewed yet, under names `gh` can be given.
fn removable(loaded: &PrDetail) -> Vec<String> {
    loaded
        .reviewers
        .iter()
        .filter(|(name, state)| {
            *state == daruda_github::ReviewerState::Requested
                && daruda_github::reviewer_name_ok(name)
        })
        .map(|(name, _)| name.clone())
        .collect()
}

/// Unresolved threads, and whether there are more than that: the threads
/// once read in full, else the detail's count of the first hundred.
fn open_threads(state: &PrDetailState, loaded: &PrDetail) -> (usize, bool) {
    match state.threads() {
        Some(Load::Ready(threads)) => (
            threads
                .threads
                .iter()
                .filter(|thread| !state.thread_work().resolved_here(&thread.id))
                .count(),
            threads.truncated,
        ),
        _ => (loaded.open_threads as usize, loaded.open_threads_more),
    }
}

/// `2 failing · 5 passed · 1 skipped`, leaving out what there is none of.
fn checks_summary(loaded: &PrDetail) -> String {
    if loaded.checks_truncated {
        let verdict =
            crate::workspace::left_dock::git_changes::pr_strip::checks_label(loaded.summary.checks)
                .unwrap_or_else(s::checks_none);
        return s::checks_truncated(verdict);
    }
    let checks = &loaded.checks;
    let count = |status| checks.iter().filter(|e| e.run.status == status).count();
    let mut parts = Vec::new();
    let failing = count(CheckStatus::Failure);
    if failing > 0 {
        parts.push(s::checks_failing(failing));
    }
    let running = count(CheckStatus::Pending);
    if running > 0 {
        parts.push(s::checks_running(running));
    }
    let passed = count(CheckStatus::Success);
    if passed > 0 {
        parts.push(s::checks_passed(passed));
    }
    let skipped = count(CheckStatus::Ignored);
    if skipped > 0 {
        parts.push(s::checks_skipped(skipped));
    }
    if parts.is_empty() {
        s::checks_none()
    } else {
        parts.join(" · ")
    }
}

fn check_row(entry: &CheckEntry, state: &PrDetailState) -> CheckRowView {
    let run = &entry.run;
    let context: Vec<&str> = [run.workflow.as_deref(), run.event.as_deref()]
        .into_iter()
        .flatten()
        .collect();
    let log = match entry.job {
        Some(job) if run.status == CheckStatus::Failure => match state.log(job) {
            Some(load) => LogSlot::Open(job, load.clone()),
            None => LogSlot::Closed(job),
        },
        _ => LogSlot::None,
    };
    CheckRowView {
        status: run.status,
        name: run.name.clone(),
        context: (!context.is_empty()).then(|| context.join(" · ")),
        duration: entry.duration.map(notification::format_duration_compact),
        log,
    }
}

fn threads_view(
    threads: &ReviewThreads,
    state: &PrDetailState,
    drafts: Option<&daruda_store::review_drafts::PrDrafts>,
    keeping: crate::github::DraftKeeping,
) -> ThreadsView {
    let replies = drafts
        .map(|drafts| drafts.replies.as_slice())
        .unwrap_or_default();
    let drafted = |thread: &str| replies.iter().any(|held| held.thread == thread);
    ThreadsView::Listed {
        keeping,
        // Kept for threads no longer listed: resolved or gone since. With
        // the list cut short, an unlisted thread may still be open.
        orphan_drafts: if threads.truncated {
            Vec::new()
        } else {
            replies
                .iter()
                .filter(|held| {
                    !threads
                        .threads
                        .iter()
                        .any(|thread| thread.id == held.thread)
                })
                .map(|held| (held.thread.clone(), held.text.clone()))
                .collect()
        },
        rows: threads
            .threads
            .iter()
            .map(|thread| {
                let mut row = thread_row(thread);
                if state.thread_expanded(&thread.id) {
                    row.comments = Some(thread.comments.iter().map(comment_view).collect());
                }
                let work = state.thread_work();
                row.resolved_here = work.resolved_here(&thread.id);
                row.draft = drafted(&thread.id);
                if let Some(work) = work.get(&thread.id) {
                    row.reply = work.reply.clone();
                    row.op = work.op.clone();
                    row.checking = work.checking();
                }
                row
            })
            .collect(),
        truncated: threads.truncated,
    }
}

fn comment_view(comment: &daruda_github::ReviewComment) -> CommentView {
    CommentView {
        id: comment.id.clone(),
        author: comment.author.clone().unwrap_or_else(s::author_deleted),
        body: comment.body.clone(),
        url: comment.web_url().map(str::to_owned),
    }
}

fn thread_row(thread: &ReviewThread) -> ThreadRowView {
    let location = match thread.lines {
        Some((first, last)) if first == last => s::thread_line(&thread.path, first),
        Some((first, last)) => s::thread_lines(&thread.path, first, last),
        None => thread.path.clone(),
    };
    let first = thread.comments.first();
    let replies = thread.comments.len().saturating_sub(1);
    ThreadRowView {
        location,
        author: first
            .and_then(|comment| comment.author.clone())
            .unwrap_or_else(s::author_deleted),
        excerpt: first
            .map(|comment| excerpt(&comment.body))
            .unwrap_or_default(),
        replies: match (replies, thread.more_comments) {
            (_, true) => Some(s::thread_replies_more(replies)),
            (0, false) => None,
            (n, false) => Some(s::thread_replies(n)),
        },
        outdated: thread.outdated,
        id: thread.id.clone(),
        url: first.and_then(|comment| comment.web_url().map(str::to_owned)),
        target: thread
            .lines
            .filter(|_| !thread.outdated)
            .map(|(start, end)| ThreadTarget {
                path: thread.path.clone(),
                side: match thread.side {
                    daruda_github::DiffSide::Left => super::diff_layout::Side::Old,
                    daruda_github::DiffSide::Right => super::diff_layout::Side::New,
                },
                start,
                end,
            }),
        comments: None,
        resolved_here: false,
        reply: None,
        op: None,
        checking: false,
        may: thread.may,
        draft: false,
    }
}

/// The first line with text in it, without control characters: a reviewer's
/// text is drawn as it came.
fn excerpt(body: &str) -> String {
    body.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control())
        .collect()
}

fn side(
    item: &PrListItem,
    loaded: &PrDetail,
    lane: Option<RowLane>,
    threads: (usize, bool),
    state: &PrDetailState,
) -> SideView {
    let (merge_state, merge_mood) = match loaded.mergeability {
        Mergeability::Ready => (s::merge_ready(), Mood::Good),
        Mergeability::Blocked(reason) => (block_label(reason), Mood::Warn),
        Mergeability::Conflicting => (s::merge_conflicting(), Mood::Bad),
        Mergeability::Unknown => (s::merge_unknown(), Mood::Quiet),
    };
    let (conflicts, conflicts_mood) = match loaded.mergeability {
        Mergeability::Conflicting => (s::conflicts_yes(), Mood::Bad),
        Mergeability::Unknown => (s::merge_unknown(), Mood::Quiet),
        Mergeability::Ready | Mergeability::Blocked(_) => (s::conflicts_none(), Mood::Good),
    };
    let methods = if loaded.methods.is_empty() {
        s::merge_methods_none()
    } else {
        let labels: Vec<String> = loaded.methods.iter().map(|m| method_label(*m)).collect();
        labels.join(", ")
    };
    // With the reviewers editable, those that can be unasked are listed
    // with their control instead.
    let unaskable = if loaded.can_update {
        removable(loaded)
    } else {
        Vec::new()
    };
    let mut reviews: Vec<Fact> = loaded
        .reviewers
        .iter()
        .filter(|(name, _)| !unaskable.contains(name))
        .map(|(name, state)| {
            let (value, mood) = reviewer_label(*state);
            Fact {
                label: name.clone(),
                value,
                mood,
            }
        })
        .collect();
    if reviews.is_empty() && unaskable.is_empty() {
        reviews.push(Fact {
            label: s::side_reviewers(),
            value: s::reviewers_none(),
            mood: Mood::Quiet,
        });
    }
    reviews.push(Fact {
        label: s::side_open_threads(),
        value: match threads {
            (count, false) => count.to_string(),
            (count, true) => s::count_capped(count),
        },
        mood: if threads == (0, false) {
            Mood::Quiet
        } else {
            Mood::Warn
        },
    });
    let lane_note = lane.as_ref().map(|_| {
        if item.pr.cross_repo {
            LaneNote::ReviewOnly
        } else {
            LaneNote::UpdatesPr
        }
    });
    SideView {
        merge: vec![
            Fact {
                label: s::side_state(),
                value: merge_state,
                mood: merge_mood,
            },
            Fact {
                label: s::side_methods(),
                value: methods,
                mood: Mood::Quiet,
            },
            Fact {
                label: s::side_conflicts(),
                value: conflicts,
                mood: conflicts_mood,
            },
        ]
        .into_iter()
        .chain(auto_merge_fact(loaded))
        .chain(queue_fact(loaded))
        .collect(),
        reviews,
        lane,
        lane_note,
        requests: loaded.can_update.then(|| {
            let requests = state.reviewer_requests();
            RequestsView {
                removable: removable(loaded),
                input: requests.input.clone(),
                op: requests.op.clone(),
            }
        }),
    }
}

/// Auto-merge, while on: with which method and who turned it on. Not a
/// merge: GitHub merges (or queues) it once its requirements pass.
fn auto_merge_fact(loaded: &PrDetail) -> Option<Fact> {
    let on = loaded.auto_merge.as_ref()?;
    let method = on.method.map_or_else(s::method_unknown, method_label);
    Some(Fact {
        label: s::side_auto_merge(),
        value: match &on.by {
            Some(by) => s::auto_merge_on_by(method, by),
            None => s::auto_merge_on(method),
        },
        mood: Mood::Good,
    })
}

/// Its place in the merge queue: accepted, not merged yet.
fn queue_fact(loaded: &PrDetail) -> Option<Fact> {
    let entry = loaded.queue?;
    let (state, mood) = match entry.state {
        QueueState::Queued => (s::queue_queued(), Mood::Quiet),
        QueueState::AwaitingChecks => (s::queue_awaiting_checks(), Mood::Quiet),
        QueueState::Mergeable => (s::queue_mergeable(), Mood::Good),
        QueueState::Unmergeable => (s::queue_unmergeable(), Mood::Bad),
        QueueState::Locked => (s::queue_locked(), Mood::Warn),
        QueueState::Other => (s::queue_in_queue(), Mood::Quiet),
    };
    Some(Fact {
        label: s::side_merge_queue(),
        value: match entry.position {
            Some(position) => s::queue_position(position, state),
            None => state,
        },
        mood,
    })
}

fn block_label(reason: BlockReason) -> String {
    match reason {
        BlockReason::Requirements => s::blocked_requirements(),
        BlockReason::Behind => s::blocked_behind(),
        BlockReason::Draft => s::blocked_draft(),
    }
}

fn method_label(method: MergeMethod) -> String {
    match method {
        MergeMethod::Merge => s::method_merge(),
        MergeMethod::Squash => s::method_squash(),
        MergeMethod::Rebase => s::method_rebase(),
    }
}

fn reviewer_label(state: ReviewerState) -> (String, Mood) {
    match state {
        ReviewerState::Requested => (s::reviewer_requested(), Mood::Quiet),
        ReviewerState::Approved => (s::review_approved(), Mood::Good),
        ReviewerState::ChangesRequested => (s::review_changes_requested(), Mood::Warn),
        ReviewerState::Commented => (s::reviewer_commented(), Mood::Quiet),
        ReviewerState::Dismissed => (s::reviewer_dismissed(), Mood::Quiet),
        ReviewerState::Pending => (s::reviewer_pending(), Mood::Quiet),
        ReviewerState::Other => (s::reviewer_other(), Mood::Quiet),
    }
}

#[cfg(test)]
mod tests;
