use std::collections::BTreeSet;
use std::time::Duration;

use daruda_github::{
    CheckKind, CheckRollup, CheckRun, FailedCheck, PrSummary, RepoId, ReviewComment, ReviewDecision,
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
        head_owner: None,
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
        default_method: None,
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
        checks_truncated: false,
        open_threads,
        open_threads_more: false,
        can_update: false,
        can_close: true,
        can_reopen: true,
        auto_merge: None,
        queue: None,
        can_enable_auto_merge: false,
        can_disable_auto_merge: false,
        node_id: None,
        changed_files: None,
    }
}

fn state(load: Load<PrDetail>, cross_repo: bool) -> PrDetailState {
    let mut state = PrDetailState::new(PrDetailId(1), item(cross_repo));
    state.pr = super::super::ReadSlot::settled(load);
    state
}

fn lane() -> LaneFacts {
    LaneFacts {
        lane: LaneRef {
            project: 1,
            lane: 2,
        },
        name: "fix".into(),
        branch: None,
        pr: None,
    }
}

static NOTHING_CREATING: BTreeSet<PrKey> = BTreeSet::new();
static NO_PROGRESS: super::super::progress::ReviewProgress =
    super::super::progress::ReviewProgress::new();

/// What the detail reads beside its state: worked on in `lane`, nothing
/// being sent or made.
fn inputs(lane: Option<&LaneFacts>) -> DetailInputs<'_> {
    DetailInputs {
        link: Default::default(),
        checkout_stage: None,
        lane,
        sending: false,
        creating: &NOTHING_CREATING,
        progress: &NO_PROGRESS,
        drafts: None,
        keeping: crate::github::DraftKeeping::Saved,
        handoff_chats: Vec::new(),
        now: Utc::now(),
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
        let read = DetailInputs {
            sending,
            ..inputs(linked.then_some(&lane))
        };
        let view = ready(view(&state, &read));
        assert_eq!(view.checks_send, checks_send, "{label}: checks");
        assert_eq!(view.threads_send, threads_send, "{label}: threads");
        assert_eq!(view.checks_send.enabled(), checks_send == SendSlot::Ready);
    }
}

#[test]
fn only_a_failed_actions_check_opens_its_log() {
    let mut state = state(Load::Ready(detail(true, 0, false)), false);
    let closed = ready(view(&state, &inputs(None)));
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

    state.logs.insert(
        (11, 22),
        super::super::ReadSlot::settled(Load::Ready("boom".into())),
    );
    let open = ready(view(&state, &inputs(None)));
    assert_eq!(
        open.checks[0].log,
        LogSlot::Open(JOB, Load::Ready("boom".into()))
    );
}

#[test]
fn a_partial_check_list_shows_the_overall_verdict_and_says_it_is_partial() {
    let cases = [
        (CheckRollup::Failing { failed: None }, "Checks failing"),
        (CheckRollup::Pending, "Checks running"),
        (CheckRollup::Passing, "Checks passed"),
    ];
    for (rollup, verdict) in cases {
        let mut loaded = detail(false, 0, false);
        loaded
            .checks
            .retain(|entry| entry.run.status == CheckStatus::Success);
        loaded.summary.checks = rollup;
        loaded.checks_truncated = true;
        let state = state(Load::Ready(loaded), false);
        let view = ready(view(&state, &inputs(None)));
        assert_eq!(
            view.checks_summary,
            format!("{verdict} · Only part of the check list is shown"),
            "{rollup:?}"
        );
        assert!(view.rerun.is_none(), "no unseen job is offered for rerun");
    }
}

#[test]
fn the_header_says_what_the_detail_knows_once_read() {
    let loading = view(&state(Load::Loading, false), &inputs(None));
    assert_eq!(loading.body, DetailBody::Loading);
    assert_eq!(loading.title, "#7 Fix it");
    let labels: Vec<_> = loading
        .tabs
        .iter()
        .map(|(_, label)| label.as_str())
        .collect();
    assert_eq!(labels, ["Checks", "Files", "Review threads", "Description"]);
    assert_eq!(loading.meta, "fix → main · alice · Changes requested");

    let read = view(
        &state(Load::Ready(detail(true, 3, false)), false),
        &inputs(None),
    );
    let labels: Vec<_> = read.tabs.iter().map(|(_, label)| label.as_str()).collect();
    assert_eq!(
        labels,
        ["Checks 4", "Files", "Review threads 3", "Description"],
        "the files are counted once read"
    );
    assert_eq!(
        read.meta,
        "fix → main · alice · Changes requested · Behind the base branch"
    );
    let ready = ready(read);
    assert_eq!(ready.description, None, "a blank body reads as none");
    assert_eq!(ready.side.lane, None);
}

#[test]
fn the_header_offers_what_github_allows_on_an_open_pull_request() {
    use ActionState::{Idle, Running};
    // (label, state, mergeability, allowed, running, button, menu, merge, ready)
    let behind = Mergeability::Blocked(BlockReason::Behind);
    let cases = [
        (
            "squash preferred",
            PrState::Open,
            Mergeability::Ready,
            vec![MergeMethod::Rebase, MergeMethod::Merge, MergeMethod::Squash],
            Idle,
            "Squash and merge",
            vec![
                "Squash and merge",
                "Create a merge commit",
                "Rebase and merge",
            ],
            ActionSlot::Ready,
            None,
        ),
        (
            "no squash",
            PrState::Open,
            Mergeability::Ready,
            vec![MergeMethod::Rebase, MergeMethod::Merge],
            Idle,
            "Create a merge commit",
            vec!["Create a merge commit", "Rebase and merge"],
            ActionSlot::Ready,
            None,
        ),
        (
            "behind: GitHub says no, the button does not",
            PrState::Open,
            behind,
            vec![MergeMethod::Squash],
            Idle,
            "Squash and merge",
            vec!["Squash and merge"],
            ActionSlot::Ready,
            None,
        ),
        (
            "not worked out yet",
            PrState::Open,
            Mergeability::Unknown,
            vec![MergeMethod::Squash],
            Idle,
            "Squash and merge",
            vec!["Squash and merge"],
            ActionSlot::Ready,
            None,
        ),
        (
            "conflicting",
            PrState::Open,
            Mergeability::Conflicting,
            vec![MergeMethod::Squash],
            Idle,
            "Squash and merge",
            vec!["Squash and merge"],
            ActionSlot::Blocked("Can’t merge: Conflicts with the base branch".into()),
            None,
        ),
        (
            "draft: ready for review first",
            PrState::Draft,
            Mergeability::Blocked(BlockReason::Draft),
            vec![MergeMethod::Squash],
            Idle,
            "Squash and merge",
            vec!["Squash and merge"],
            ActionSlot::Blocked("Can’t merge: Draft".into()),
            Some(ActionSlot::Ready),
        ),
        (
            "a draft only by why it cannot merge is one all the same",
            PrState::Open,
            Mergeability::Blocked(BlockReason::Draft),
            vec![MergeMethod::Squash],
            Idle,
            "Squash and merge",
            vec!["Squash and merge"],
            ActionSlot::Blocked("Can’t merge: Draft".into()),
            Some(ActionSlot::Ready),
        ),
        (
            "no method",
            PrState::Open,
            Mergeability::Ready,
            vec![],
            Idle,
            "Merge",
            vec![],
            ActionSlot::Blocked(s::merge_no_methods()),
            None,
        ),
        (
            "draft, merging",
            PrState::Draft,
            Mergeability::Ready,
            vec![MergeMethod::Squash],
            Running(PrAction::Merging),
            "Squash and merge",
            vec!["Squash and merge"],
            ActionSlot::Running,
            Some(ActionSlot::Waiting),
        ),
    ];
    for (label, pr_state, mergeability, allowed, running, button, menu, merge, ready) in cases {
        let mut loaded = detail(false, 0, false);
        loaded.summary.state = pr_state;
        loaded.mergeability = mergeability;
        loaded.methods = allowed;
        let mut state = state(Load::Ready(loaded), false);
        state.action = running.clone();
        let view = view(&state, &inputs(None));
        let draft = if ready.is_some() { "Draft" } else { "Open" };
        assert_eq!(view.state, draft, "{label}: state");
        let actions = view.header.actions().cloned().expect(label);
        assert_eq!(actions.merge.label, button, "{label}: button");
        let labels: Vec<_> = actions
            .merge
            .methods
            .iter()
            .map(|(_, l)| l.as_str())
            .collect();
        assert_eq!(labels, menu, "{label}: menu");
        assert_eq!(actions.merge.slot, merge, "{label}: merge");
        assert_eq!(actions.ready, ready, "{label}: ready");
    }

    for done in [PrState::Merged, PrState::Closed] {
        let mut loaded = detail(false, 0, false);
        loaded.summary.state = done;
        let state = state(Load::Ready(loaded), false);
        assert_eq!(
            view(&state, &inputs(None)).header.actions().cloned(),
            None,
            "{done:?}"
        );
    }
    let unread = view(&state(Load::Loading, false), &inputs(None));
    assert_eq!(
        unread.header.actions(),
        None,
        "nothing to act on before it is read"
    );
}

#[test]
fn the_linked_worktree_says_whether_its_pushes_reach_the_pull_request() {
    let lane = lane();
    for (cross_repo, note) in [(true, LaneNote::ReviewOnly), (false, LaneNote::UpdatesPr)] {
        let state = state(Load::Ready(detail(false, 0, cross_repo)), cross_repo);
        let side = ready(view(&state, &inputs(Some(&lane)))).side;
        assert_eq!(side.lane_note, Some(note), "cross_repo {cross_repo}");
        assert_eq!(side.lane.map(|lane| lane.name), Some("fix".to_owned()));
    }
    let state = state(Load::Ready(detail(false, 0, false)), false);
    let side = ready(view(&state, &inputs(None))).side;
    assert_eq!(side.lane_note, None, "nothing to say without a worktree");
}

#[test]
fn without_a_worktree_the_header_offers_to_make_one() {
    let lane = lane();
    let linked = RowLane {
        lane: lane.lane,
        name: "fix".into(),
    };
    // (label, cross_repo, linked, creating, slot)
    let cases = [
        ("linked", false, true, false, LaneSlot::Linked(linked)),
        (
            "being made",
            false,
            false,
            true,
            LaneSlot::Creating { review: false },
        ),
        (
            "a fork's, being made: still a review worktree",
            true,
            false,
            true,
            LaneSlot::Creating { review: true },
        ),
        (
            "same repository",
            false,
            false,
            false,
            LaneSlot::Absent { review: false },
        ),
        (
            "a fork: a review worktree",
            true,
            false,
            false,
            LaneSlot::Absent { review: true },
        ),
    ];
    for (label, cross_repo, linked, creating, slot) in cases {
        let state = state(Load::Loading, cross_repo);
        let made: BTreeSet<PrKey> = creating
            .then(|| (state.item.repo.clone(), state.item.pr.number))
            .into_iter()
            .collect();
        let read = DetailInputs {
            creating: &made,
            ..inputs(linked.then_some(&lane))
        };
        let view = view(&state, &read);
        assert_eq!(view.lane, slot, "{label}");
    }
}

#[test]
fn a_failed_action_is_said_under_the_header_and_frees_the_buttons() {
    let mut state = state(Load::Ready(detail(true, 0, false)), false);
    assert_eq!(view(&state, &inputs(None)).error(), None);
    state.action = ActionState::Failed("Merging #7 failed: nope".into());
    let failed = view(&state, &inputs(None));
    assert_eq!(failed.error().as_deref(), Some("Merging #7 failed: nope"));
    let actions = failed.header.actions().cloned().expect("open");
    assert_eq!(actions.close, ActionSlot::Ready);
    assert_eq!(ready(failed).rerun, Some(ActionSlot::Ready));
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
                ..Default::default()
            },
            ReviewComment {
                author: None,
                body: "ok".into(),
                ..Default::default()
            },
        ],
        more_comments: false,
        ..Default::default()
    };
    let row = thread_row(&thread);
    assert_eq!(row.location, "src/a.rs:L3–L5");
    assert_eq!(row.excerpt, "Rename this.");
    assert_eq!(row.author, "bob");
    assert_eq!(row.replies.as_deref(), Some("1 replies"));
    assert!(row.outdated);
}
