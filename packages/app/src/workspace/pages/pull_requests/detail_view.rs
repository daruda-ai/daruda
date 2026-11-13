//! What the detail draws, built from plain values so every case is testable
//! without a window: the header, each tab's content, the summary column, and
//! whether Send to agent can go anywhere.

use chrono::{DateTime, Utc};
use daruda_github::{
    ActionsJob, BlockReason, CheckEntry, CheckStatus, MergeMethod, Mergeability, PrDetail,
    PrListItem, PrState, ReviewThread, ReviewThreads, ReviewerState,
};

use super::detail::{Load, PrDetailId, PrDetailState, PrTab};
use super::list::{self, LaneFacts, RowLane};
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
    /// `path:L12–L20`.
    pub location: String,
    pub author: String,
    /// The first line of the first comment.
    pub excerpt: String,
    pub replies: Option<String>,
    pub outdated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) enum ThreadsView {
    Loading,
    Failed(String),
    Listed {
        rows: Vec<ThreadRowView>,
        truncated: bool,
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
    /// Said under the worktree: a fork's review worktree cannot push to it.
    pub lane_note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct ReadyView {
    pub checks_summary: String,
    pub checks: Vec<CheckRowView>,
    pub checks_send: SendSlot,
    pub threads: ThreadsView,
    pub threads_send: SendSlot,
    /// `None` when the author wrote none.
    pub description: Option<String>,
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
    /// `Pull requests / owner/name`.
    pub crumb: String,
    pub state: String,
    pub state_mood: Mood,
    /// `#480 Adjust sidebar…`.
    pub title: String,
    /// `head → base · author · 2h ago · Changes requested · Behind the base`.
    pub meta: String,
    pub url: Option<String>,
    pub lane: Option<RowLane>,
    pub tab: PrTab,
    pub tabs: Vec<(PrTab, String)>,
    pub body: DetailBody,
}

/// The detail for `state`, worked on in `lane` (if any), whose sends are
/// under way when `sending`.
pub(in crate::workspace) fn view(
    state: &PrDetailState,
    lane: Option<&LaneFacts>,
    sending: bool,
    now: DateTime<Utc>,
) -> PrDetailView {
    let item = &state.item;
    let loaded = match &state.load {
        Load::Ready(loaded) => Some(loaded),
        Load::Loading | Load::Failed(_) => None,
    };
    let pr = loaded.map_or(&item.pr, |loaded| &loaded.summary);
    let row_lane = lane.map(|lane| RowLane {
        lane: lane.lane,
        name: lane.name.clone(),
    });
    let (state_label, state_mood) = state_label(pr.state);
    PrDetailView {
        id: state.id,
        crumb: format!(
            "{} / {}/{}",
            s::title(),
            item.repo.owner(),
            item.repo.name()
        ),
        state: state_label,
        state_mood,
        title: format!("{} {}", git_s::pr_number(pr.number), pr.title),
        meta: meta(item, loaded, now),
        url: pr.web_url().map(str::to_owned),
        lane: row_lane.clone(),
        tab: state.tab,
        tabs: PrTab::ALL
            .into_iter()
            .map(|tab| (tab, tab_label(tab, loaded)))
            .collect(),
        body: match &state.load {
            Load::Loading => DetailBody::Loading,
            Load::Failed(reason) => DetailBody::Failed(reason.clone()),
            Load::Ready(loaded) => {
                DetailBody::Ready(Box::new(ready(state, item, loaded, row_lane, sending)))
            }
        },
    }
}

fn ready(
    state: &PrDetailState,
    item: &PrListItem,
    loaded: &PrDetail,
    lane: Option<RowLane>,
    sending: bool,
) -> ReadyView {
    let send = |nothing: bool| match (&lane, sending, nothing) {
        (None, _, _) => SendSlot::NoLane,
        (Some(_), true, _) => SendSlot::Sending,
        (Some(_), false, true) => SendSlot::Nothing,
        (Some(_), false, false) => SendSlot::Ready,
    };
    ReadyView {
        checks_summary: checks_summary(&loaded.checks),
        checks: loaded
            .checks
            .iter()
            .map(|entry| check_row(entry, state))
            .collect(),
        checks_send: send(loaded.summary.failed.is_empty()),
        threads: match &state.threads {
            None | Some(Load::Loading) => ThreadsView::Loading,
            Some(Load::Failed(reason)) => ThreadsView::Failed(reason.clone()),
            Some(Load::Ready(threads)) => threads_view(threads),
        },
        threads_send: send(loaded.open_threads == 0),
        description: Some(loaded.body.trim())
            .filter(|body| !body.is_empty())
            .map(str::to_owned),
        side: side(item, loaded, lane),
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
    if let Some(Mergeability::Blocked(reason)) = loaded.map(|loaded| loaded.mergeability) {
        parts.push(block_label(reason));
    }
    if loaded.is_some_and(|loaded| loaded.mergeability == Mergeability::Conflicting) {
        parts.push(s::merge_conflicting());
    }
    parts.join(" · ")
}

fn tab_label(tab: PrTab, loaded: Option<&PrDetail>) -> String {
    let (label, count) = match tab {
        PrTab::Checks => (s::tab_checks(), loaded.map(|l| l.checks.len())),
        PrTab::Threads => (s::tab_threads(), loaded.map(|l| l.open_threads as usize)),
        PrTab::Description => return s::tab_description(),
    };
    match count {
        Some(count) => s::tab_count(label, count),
        None => label,
    }
}

/// `2 failing · 5 passed · 1 skipped`, leaving out what there is none of.
fn checks_summary(checks: &[CheckEntry]) -> String {
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

fn threads_view(threads: &ReviewThreads) -> ThreadsView {
    ThreadsView::Listed {
        rows: threads.threads.iter().map(thread_row).collect(),
        truncated: threads.truncated,
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

fn side(item: &PrListItem, loaded: &PrDetail, lane: Option<RowLane>) -> SideView {
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
    let mut reviews: Vec<Fact> = loaded
        .reviewers
        .iter()
        .map(|(name, state)| {
            let (value, mood) = reviewer_label(*state);
            Fact {
                label: name.clone(),
                value,
                mood,
            }
        })
        .collect();
    if reviews.is_empty() {
        reviews.push(Fact {
            label: s::side_reviewers(),
            value: s::reviewers_none(),
            mood: Mood::Quiet,
        });
    }
    reviews.push(Fact {
        label: s::side_open_threads(),
        value: loaded.open_threads.to_string(),
        mood: if loaded.open_threads == 0 {
            Mood::Quiet
        } else {
            Mood::Warn
        },
    });
    let lane_note = lane
        .as_ref()
        .filter(|_| item.pr.cross_repo)
        .map(|_| s::fork_lane_note());
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
        ],
        reviews,
        lane,
        lane_note,
    }
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
mod tests {
    use std::time::Duration;

    use daruda_github::{
        CheckKind, CheckRollup, CheckRun, FailedCheck, PrSummary, RepoId, ReviewComment,
        ReviewDecision,
    };
    use daruda_store::project::LaneRef;

    use super::*;

    fn summary(cross_repo: bool, failed: Vec<FailedCheck>) -> PrSummary {
        PrSummary {
            number: 7,
            url: "https://github.com/o/r/pull/7".into(),
            title: "Fix it".into(),
            state: PrState::Open,
            head_branch: "fix".into(),
            cross_repo,
            checks: CheckRollup::Passing,
            failed,
            review: Some(ReviewDecision::ChangesRequested),
            updated_at: String::new(),
        }
    }

    fn item(cross_repo: bool) -> PrListItem {
        PrListItem {
            repo: RepoId::new("github.com", "o", "r"),
            pr: summary(cross_repo, Vec::new()),
            author: Some("alice".into()),
            base_branch: "main".into(),
            comments: 0,
        }
    }

    fn run(name: &str, status: CheckStatus, workflow: Option<&str>) -> CheckRun {
        CheckRun {
            kind: CheckKind::Run,
            workflow: workflow.map(str::to_owned),
            event: workflow.map(|_| "pull_request".to_owned()),
            name: name.into(),
            status,
            details_url: None,
            started_at: None,
            completed_at: None,
        }
    }

    const JOB: ActionsJob = ActionsJob { run: 11, job: 22 };

    fn detail(failed: bool, open_threads: u32, cross_repo: bool) -> PrDetail {
        let failure = FailedCheck {
            workflow: Some("CI".into()),
            event: Some("pull_request".into()),
            name: "test".into(),
            details_url: None,
            job: Some(JOB),
        };
        PrDetail {
            summary: summary(cross_repo, if failed { vec![failure] } else { Vec::new() }),
            body: "  ".into(),
            head_sha: "abc".into(),
            base_branch: "main".into(),
            author: Some("alice".into()),
            mergeability: Mergeability::Blocked(BlockReason::Behind),
            methods: vec![MergeMethod::Squash],
            reviewers: vec![("bob".into(), ReviewerState::ChangesRequested)],
            checks: vec![
                CheckEntry {
                    run: run("test", CheckStatus::Failure, Some("CI")),
                    job: Some(JOB),
                    duration: Some(Duration::from_secs(83)),
                },
                CheckEntry {
                    run: run("prow", CheckStatus::Failure, None),
                    job: None,
                    duration: None,
                },
                CheckEntry {
                    run: run("lint", CheckStatus::Success, Some("CI")),
                    job: Some(ActionsJob { run: 11, job: 23 }),
                    duration: None,
                },
                CheckEntry {
                    run: run("docs", CheckStatus::Ignored, None),
                    job: None,
                    duration: None,
                },
            ],
            open_threads,
        }
    }

    fn state(load: Load<PrDetail>, cross_repo: bool) -> PrDetailState {
        let mut state = PrDetailState::new(PrDetailId(1), item(cross_repo));
        state.load = load;
        state
    }

    fn lane() -> LaneFacts {
        LaneFacts {
            lane: LaneRef {
                project: 1,
                lane: 2,
            },
            name: "fix".into(),
            repo: None,
            branch: None,
            pr: None,
        }
    }

    fn ready(view: PrDetailView) -> ReadyView {
        match view.body {
            DetailBody::Ready(ready) => *ready,
            other => panic!("not ready: {other:?}"),
        }
    }

    #[test]
    fn send_to_agent_needs_a_linked_worktree_and_something_to_send() {
        // (label, linked, sending, failing check, open threads, checks, threads)
        let cases = [
            (
                "no worktree",
                false,
                false,
                true,
                2,
                SendSlot::NoLane,
                SendSlot::NoLane,
            ),
            (
                "ready",
                true,
                false,
                true,
                2,
                SendSlot::Ready,
                SendSlot::Ready,
            ),
            (
                "sending",
                true,
                true,
                true,
                2,
                SendSlot::Sending,
                SendSlot::Sending,
            ),
            (
                "nothing",
                true,
                false,
                false,
                0,
                SendSlot::Nothing,
                SendSlot::Nothing,
            ),
        ];
        for (label, linked, sending, failed, threads, checks_send, threads_send) in cases {
            let lane = lane();
            let state = state(Load::Ready(detail(failed, threads, false)), false);
            let view = ready(view(&state, linked.then_some(&lane), sending, Utc::now()));
            assert_eq!(view.checks_send, checks_send, "{label}: checks");
            assert_eq!(view.threads_send, threads_send, "{label}: threads");
            assert_eq!(view.checks_send.enabled(), checks_send == SendSlot::Ready);
        }
    }

    #[test]
    fn only_a_failed_actions_check_opens_its_log() {
        let mut state = state(Load::Ready(detail(true, 0, false)), false);
        let closed = ready(view(&state, None, false, Utc::now()));
        let logs: Vec<_> = closed.checks.iter().map(|row| row.log.clone()).collect();
        assert_eq!(
            logs,
            [
                LogSlot::Closed(JOB),
                LogSlot::None,
                LogSlot::None,
                LogSlot::None
            ],
            "a commit status and a passing check have no log to show"
        );
        assert_eq!(
            closed.checks[0].context.as_deref(),
            Some("CI · pull_request")
        );
        assert_eq!(closed.checks[0].duration.as_deref(), Some("1m 23s"));
        assert_eq!(closed.checks_summary, "2 failing · 1 passed · 1 skipped");

        state.logs.insert((11, 22), Load::Ready("boom".into()));
        let open = ready(view(&state, None, false, Utc::now()));
        assert_eq!(
            open.checks[0].log,
            LogSlot::Open(JOB, Load::Ready("boom".into()))
        );
    }

    #[test]
    fn the_header_says_what_the_detail_knows_once_read() {
        let loading = view(&state(Load::Loading, false), None, false, Utc::now());
        assert_eq!(loading.body, DetailBody::Loading);
        assert_eq!(loading.crumb, "Pull requests / o/r");
        assert_eq!(loading.title, "#7 Fix it");
        let labels: Vec<_> = loading
            .tabs
            .iter()
            .map(|(_, label)| label.as_str())
            .collect();
        assert_eq!(labels, ["Checks", "Review threads", "Description"]);
        assert_eq!(loading.meta, "fix → main · alice · Changes requested");

        let read = view(
            &state(Load::Ready(detail(true, 3, false)), false),
            None,
            false,
            Utc::now(),
        );
        let labels: Vec<_> = read.tabs.iter().map(|(_, label)| label.as_str()).collect();
        assert_eq!(labels, ["Checks 4", "Review threads 3", "Description"]);
        assert_eq!(
            read.meta,
            "fix → main · alice · Changes requested · Behind the base branch"
        );
        let ready = ready(read);
        assert_eq!(ready.description, None, "a blank body reads as none");
        assert_eq!(ready.side.lane, None);
    }

    #[test]
    fn a_forks_review_worktree_says_its_pushes_go_nowhere() {
        let lane = lane();
        for (cross_repo, note) in [(true, Some(s::fork_lane_note())), (false, None)] {
            let state = state(Load::Ready(detail(false, 0, cross_repo)), cross_repo);
            let side = ready(view(&state, Some(&lane), false, Utc::now())).side;
            assert_eq!(side.lane_note, note, "cross_repo {cross_repo}");
            assert_eq!(side.lane.map(|lane| lane.name), Some("fix".to_owned()));
        }
    }

    #[test]
    fn a_thread_reads_as_its_place_first_words_and_replies() {
        let thread = ReviewThread {
            path: "src/a.rs".into(),
            lines: Some((3, 5)),
            outdated: true,
            comments: vec![
                ReviewComment {
                    author: Some("bob".into()),
                    body: "\n  Rename this\u{7}.\nIt hides the cause.".into(),
                },
                ReviewComment {
                    author: None,
                    body: "ok".into(),
                },
            ],
            more_comments: false,
        };
        let row = thread_row(&thread);
        assert_eq!(row.location, "src/a.rs:L3–L5");
        assert_eq!(row.excerpt, "Rename this.");
        assert_eq!(row.author, "bob");
        assert_eq!(row.replies.as_deref(), Some("1 replies"));
        assert!(row.outdated);
    }
}
