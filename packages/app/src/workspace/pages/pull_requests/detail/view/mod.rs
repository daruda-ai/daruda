//! What the detail draws, built from plain values so every case is testable
//! without a window: the header, each tab's content, the summary column, and
//! whether Send to agent can go anywhere.

use chrono::{DateTime, Utc};
use daruda_github::{
    ActionsJob, BlockReason, CheckEntry, CheckStatus, MergeBlock, MergeMethod, Mergeability,
    PrDetail, PrListItem, PrState, QueueState, ReviewThread, ReviewThreads, ReviewerState,
};

use super::super::actions::{self, ActionState, PrAction};
use super::super::list::{self, LaneFacts, LaneSlot, RowLane};
use super::{Load, PrDetailId, PrDetailState, PrTab};
use crate::surface::strings::{git as git_s, notification, pull_requests as s};

mod checks;
mod discussion;
mod header;
mod side;
mod threads;

pub(in crate::workspace) use checks::*;
pub(in crate::workspace) use discussion::*;
pub(in crate::workspace) use header::*;
pub(in crate::workspace) use side::*;
pub(in crate::workspace) use threads::*;

/// How a value reads: the colour it is drawn in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum Mood {
    Good,
    Warn,
    Bad,
    Quiet,
}

/// What the last action came to, under the header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) enum ActionNotice {
    Failed(String),
    Done(String),
}

#[cfg(test)]
impl PrDetailView {
    /// The failure said under the header, for a test.
    pub(in crate::workspace) fn error(&self) -> Option<String> {
        match &self.notice {
            Some(ActionNotice::Failed(line)) => Some(line.clone()),
            Some(ActionNotice::Done(_)) | None => None,
        }
    }
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
    /// How far making its worktree has got, while it runs.
    pub checkout_stage: Option<super::super::CheckoutStage>,
    pub header: HeaderControls,
    /// What the last action came to, until the next action or reload.
    pub notice: Option<ActionNotice>,
    pub tab: PrTab,
    pub tabs: Vec<(PrTab, String)>,
    pub handoff: Option<HandoffView>,
    pub body: DetailBody,
}

/// What the detail reads beside its own state.
pub(in crate::workspace) struct DetailInputs<'a> {
    /// The worktree working on the pull request, if any.
    pub lane: Option<&'a LaneFacts>,
    /// Why it is tied to it, and the worktrees it could be tied to instead.
    pub link: super::links::LinkInputs,
    /// Whether a send to that worktree's chat is under way.
    pub sending: bool,
    /// Pull requests a worktree is being made for.
    pub creating: &'a super::super::lane::Creating,
    /// How far making this one's worktree has got.
    pub checkout_stage: Option<super::super::CheckoutStage>,
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
        checkout_stage: inputs.checkout_stage,
        header: loaded.map_or(HeaderControls::None, |loaded| {
            header_controls(loaded, &state.action, stack_range(state))
        }),
        notice: match &state.action {
            ActionState::Failed(line) => Some(ActionNotice::Failed(line.clone())),
            ActionState::Done(line) => Some(ActionNotice::Done(line.clone())),
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
            can_edit: loaded.can_edit(),
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
        side: side(
            item,
            loaded,
            lane,
            open_threads(state, loaded),
            state,
            inputs.link.clone(),
        ),
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

#[cfg(test)]
mod tests;
