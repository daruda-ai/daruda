//! A pull request's detail on the Pull requests page: what it reads from
//! GitHub, what it drops, where Send to agent goes, and what its merge,
//! close, ready and rerun actions ask GitHub.

use daruda_github::fake::{FakeGh, GhCall, GhFailure, GhOp};
use daruda_github::{
    ActionsJob, CheckEntry, CheckKind, CheckRollup, CheckRun, CheckStatus, FailedCheck,
    MergeMethod, Mergeability, PrDetail, PrListItem, PrState, PrSummary, RepoId,
};
use daruda_store::project::LaneRef;
use gpui::{AppContext as _, TestAppContext, VisualTestContext};

use crate::lane::git::{FakeRepo, PushTarget};
use crate::workspace::pages::Page;
use crate::workspace::pages::pull_requests::detail::{Load, PrDetailId};
use crate::workspace::pages::pull_requests::detail_view::{
    ActionSlot, DetailBody, LogSlot, SendSlot,
};
use crate::workspace::tests::GitFixture;

const JOB: ActionsJob = ActionsJob { run: 11, job: 22 };

fn repo() -> RepoId {
    RepoId::new("github.com", "o", "r")
}

fn summary(number: u64, head: &str) -> PrSummary {
    PrSummary {
        number,
        url: format!("https://github.com/o/r/pull/{number}"),
        title: format!("PR {number}"),
        state: PrState::Open,
        head_branch: head.into(),
        cross_repo: false,
        checks: CheckRollup::Failing { failed: Some(1) },
        failed: vec![FailedCheck {
            workflow: Some("CI".into()),
            event: Some("pull_request".into()),
            name: "test".into(),
            details_url: Some("https://github.com/o/r/actions/runs/11/job/22".into()),
            job: Some(JOB),
        }],
        review: None,
        updated_at: "2026-01-01T00:00:00Z".into(),
    }
}

fn item(number: u64, head: &str) -> PrListItem {
    PrListItem {
        repo: repo(),
        pr: summary(number, head),
        author: Some("alice".into()),
        head_owner: None,
        base_branch: "main".into(),
        comments: 0,
    }
}

fn detail(number: u64, head: &str) -> PrDetail {
    PrDetail {
        summary: summary(number, head),
        body: "Body".into(),
        head_sha: "abc".into(),
        base_branch: "main".into(),
        author: Some("alice".into()),
        mergeability: Mergeability::Ready,
        methods: Vec::new(),
        default_method: None,
        reviewers: Vec::new(),
        checks: vec![CheckEntry {
            run: CheckRun {
                kind: CheckKind::Run,
                workflow: Some("CI".into()),
                event: Some("pull_request".into()),
                name: "test".into(),
                status: CheckStatus::Failure,
                details_url: None,
                started_at: None,
                completed_at: None,
            },
            job: Some(JOB),
            duration: None,
        }],
        open_threads: 0,
    }
}

/// A window whose one lane pushes `feat` to o/r, linked to PR #7, which
/// GitHub can describe, with one failing Actions job whose log it prints.
fn window(cx: &mut TestAppContext) -> (GitFixture, FakeGh, LaneRef) {
    let gh = crate::test_support::fake_gh(cx);
    gh.set_prs(&repo(), vec![summary(7, "feat")]);
    gh.set_pr_detail(&repo(), 7, detail(7, "feat"));
    gh.set_failed_log(
        JOB,
        "job\tstep\t2026-01-01T00:00:00.0000000Z ##[group]Run cargo test\n\
         job\tstep\t2026-01-01T00:00:01.0000000Z test parse ... \u{1b}[31mFAILED\u{1b}[0m\n\
         job\tstep\t2026-01-01T00:00:02.0000000Z ##[error]Process completed with exit code 101.",
    );
    let f = GitFixture::open(cx, &daruda_config::Config::default(), |repo: FakeRepo| {
        repo.push_target(PushTarget {
            remote: "origin".into(),
            url: "git@github.com:o/r.git".into(),
            branch: "feat".into(),
            published: true,
            has_upstream: false,
        })
    });
    let linked = f.ws.read_with(cx, |ws, _| ws.active_ref());
    f.ws.update(cx, |ws, cx| ws.refresh_git_status(linked, cx));
    cx.run_until_parked();
    let (window, ws) = (f.window, f.ws.downgrade());
    cx.update(|cx| crate::window_registry::WindowRegistry::register(window.into(), ws, cx));
    cx.update(crate::github::PrPoller::tick);
    cx.run_until_parked();
    f.ws.update(cx, |ws, cx| ws.show_page(Page::PullRequests, cx));
    (f, gh, linked)
}

/// Click a row, as the page does, without waiting for GitHub.
fn open(f: &GitFixture, item: PrListItem, cx: &mut TestAppContext) {
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| ws.open_pr_detail(item, window, cx));
    });
}

fn shown(f: &GitFixture, cx: &mut TestAppContext) -> Option<(PrDetailId, u64, Load<PrDetail>)> {
    f.ws.read_with(cx, |ws, _| {
        let detail = ws.pages.pull_requests.detail.as_ref()?;
        Some((detail.id, detail.item.pr.number, detail.load.clone()))
    })
}

fn id(f: &GitFixture, cx: &mut TestAppContext) -> PrDetailId {
    shown(f, cx).expect("a detail is shown").0
}

fn view(
    f: &GitFixture,
    cx: &mut TestAppContext,
) -> crate::workspace::pages::pull_requests::detail_view::PrDetailView {
    f.ws.update(cx, |ws, cx| {
        ws.stage_pr_page(cx);
        ws.pages.pull_requests.detail_snapshot.clone()
    })
    .expect("the page shows its detail")
}

fn input(f: &GitFixture, cx: &mut TestAppContext) -> String {
    f.ws.read_with(cx, |ws, cx| {
        ws.input_dock.input.read(cx).value().to_string()
    })
}

fn split_chat(f: &GitFixture, cx: &mut TestAppContext) {
    cx.update_window(f.window.into(), |_, window, cx| {
        f.ws.update(cx, |ws, cx| {
            ws.split_focused_pane_kind(
                super::NewPaneKind::AgentChat,
                super::SplitDirection::Horizontal,
                window,
                cx,
            );
        });
    })
    .unwrap();
}

#[gpui::test]
fn opening_a_detail_reads_the_pull_request_once(cx: &mut TestAppContext) {
    let (f, gh, _) = window(cx);
    open(&f, item(7, "feat"), cx);
    cx.run_until_parked();
    assert_eq!(gh.count(GhOp::PrDetail), 1);
    assert!(
        matches!(shown(&f, cx), Some((_, 7, Load::Ready(_)))),
        "{:?}",
        shown(&f, cx)
    );
    assert_eq!(
        f.ws.read_with(cx, |ws, _| ws.page_detail_id(Page::PullRequests)),
        Some(crate::workspace::pages::detail::PageDetailId::PullRequest(
            id(&f, cx)
        ))
    );

    // The same pull request again is shown as it is, not read again.
    open(&f, item(7, "feat"), cx);
    cx.run_until_parked();
    assert_eq!(gh.count(GhOp::PrDetail), 1);
}

#[gpui::test]
fn a_read_landing_after_its_detail_was_replaced_or_closed_is_dropped(cx: &mut TestAppContext) {
    let (f, gh, _) = window(cx);
    // #7 answers; #8 is unknown to GitHub and fails. Replaced before either
    // lands, the shown #8 must not take #7's answer.
    open(&f, item(7, "feat"), cx);
    let first = id(&f, cx);
    open(&f, item(8, "other"), cx);
    let second = id(&f, cx);
    assert_ne!(first, second, "ids are never reused");
    cx.run_until_parked();
    assert_eq!(gh.count(GhOp::PrDetail), 2);
    assert!(
        matches!(shown(&f, cx), Some((id, 8, Load::Failed(_))) if id == second),
        "{:?}",
        shown(&f, cx)
    );

    // Closed before its answer lands: nothing comes back.
    open(&f, item(7, "feat"), cx);
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.update(|window, cx| f.ws.update(cx, |ws, cx| ws.back_to_pr_list(window, cx)));
    cx.run_until_parked();
    assert_eq!(shown(&f, cx).map(|(_, number, _)| number), None);
}

#[gpui::test]
fn a_failed_check_opens_its_cleaned_log(cx: &mut TestAppContext) {
    let (f, gh, _) = window(cx);
    open(&f, item(7, "feat"), cx);
    cx.run_until_parked();
    let id = id(&f, cx);
    f.ws.update(cx, |ws, cx| ws.toggle_pr_check_log(id, JOB, cx));
    cx.run_until_parked();
    assert_eq!(gh.count(GhOp::FailedLog), 1);
    let DetailBody::Ready(ready) = view(&f, cx).body else {
        panic!("not read");
    };
    let LogSlot::Open(_, Load::Ready(log)) = &ready.checks[0].log else {
        panic!("{:?}", ready.checks[0].log);
    };
    assert!(log.contains("test parse ... FAILED"), "{log}");
    assert!(!log.contains('\u{1b}'), "no escapes reach the page: {log}");

    f.ws.update(cx, |ws, cx| ws.toggle_pr_check_log(id, JOB, cx));
    let DetailBody::Ready(ready) = view(&f, cx).body else {
        panic!("not read");
    };
    assert_eq!(ready.checks[0].log, LogSlot::Closed(JOB));
}

#[gpui::test]
fn send_to_agent_goes_to_the_linked_worktrees_chat(cx: &mut TestAppContext) {
    let (f, gh, linked) = window(cx);
    split_chat(&f, cx);
    // Standing on another worktree shows the prompt goes to the linked one.
    let other_dir = f.root.parent().unwrap().join("other");
    std::fs::create_dir_all(&other_dir).unwrap();
    let other = LaneRef {
        project: linked.project,
        lane: 77,
    };
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| {
            let lane = crate::lane::Lane::default_for_project(77, other_dir.clone());
            ws.project_for_mut(linked.project).unwrap().lanes.push(lane);
            ws.activate_lane(other, window, cx);
            ws.show_page(Page::PullRequests, cx);
        })
    });
    open(&f, item(7, "feat"), cx);
    cx.run_until_parked();
    let DetailBody::Ready(ready) = view(&f, cx).body else {
        panic!("not read");
    };
    assert_eq!(ready.checks_send, SendSlot::Ready);
    let id = id(&f, cx);

    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| ws.send_pr_detail_checks(id, window, cx));
    });
    cx.run_until_parked();

    assert_eq!(gh.count(GhOp::FailedLog), 1);
    f.ws.read_with(cx, |ws, _| {
        assert_eq!(ws.active_ref(), linked, "the linked worktree is shown");
        assert_eq!(ws.active_page(), None, "so its chat input is in view");
        assert!(ws.pr_detail_id().is_some(), "the detail waits on the page");
    });
    let input = input(&f, cx);
    assert!(input.contains("Pull request #7"), "{input}");
    assert!(input.contains("test parse ... FAILED"), "{input}");
}

#[gpui::test]
fn without_a_linked_worktree_send_to_agent_is_off(cx: &mut TestAppContext) {
    let (f, gh, _) = window(cx);
    split_chat(&f, cx);
    f.ws.update(cx, |ws, cx| ws.show_page(Page::PullRequests, cx));
    gh.set_pr_detail(&repo(), 8, detail(8, "other"));
    open(&f, item(8, "other"), cx);
    cx.run_until_parked();
    let DetailBody::Ready(ready) = view(&f, cx).body else {
        panic!("not read");
    };
    assert_eq!(ready.checks_send, SendSlot::NoLane);
    assert_eq!(ready.threads_send, SendSlot::NoLane);
    assert!(!ready.checks_send.enabled());

    let id = id(&f, cx);
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| ws.send_pr_detail_checks(id, window, cx));
    });
    cx.run_until_parked();
    assert_eq!(gh.count(GhOp::FailedLog), 0);
    assert_eq!(input(&f, cx), "");
    assert_eq!(
        f.ws.read_with(cx, |ws, _| ws.active_page()),
        Some(Page::PullRequests)
    );
}

/// A detail GitHub lets merge with a merge commit or a squash, whose failed
/// checks are two jobs of run 11 and one of run 12.
fn mergeable(number: u64, head: &str) -> PrDetail {
    let mut detail = detail(number, head);
    detail.methods = vec![MergeMethod::Merge, MergeMethod::Squash];
    let failed = |job: ActionsJob| {
        let mut entry = detail.checks[0].clone();
        entry.job = Some(job);
        entry
    };
    detail.checks = vec![
        failed(JOB),
        failed(ActionsJob { run: 11, job: 23 }),
        failed(ActionsJob { run: 12, job: 30 }),
    ];
    detail
}

/// The detail of #7, read, with `detail` as GitHub's answer.
fn opened(cx: &mut TestAppContext, detail: PrDetail) -> (GitFixture, FakeGh, PrDetailId) {
    let (f, gh, _) = window(cx);
    gh.set_pr_detail(&repo(), 7, detail);
    open(&f, item(7, "feat"), cx);
    cx.run_until_parked();
    let id = id(&f, cx);
    (f, gh, id)
}

/// Run `ask` in the window and say whether it raised a dialog.
fn asks(
    f: &GitFixture,
    cx: &mut TestAppContext,
    ask: impl FnOnce(&mut super::Workspace, &mut gpui::Window, &mut gpui::Context<super::Workspace>),
) -> bool {
    use crate::ui::WindowExt as _;
    cx.update_window(f.window.into(), |_, window, cx| {
        f.ws.update(cx, |ws, cx| ask(ws, window, cx));
        window.has_active_dialog(cx)
    })
    .unwrap()
}

/// The dialog's OK.
fn confirm(f: &GitFixture, cx: &mut TestAppContext) {
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.run_until_parked();
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
}

fn merges(gh: &FakeGh) -> Vec<GhCall> {
    gh.calls()
        .into_iter()
        .filter(|call| matches!(call, GhCall::MergePr { .. }))
        .collect()
}

fn loaded_state(f: &GitFixture, cx: &mut TestAppContext) -> Option<PrState> {
    match shown(f, cx)?.2 {
        Load::Ready(loaded) => Some(loaded.summary.state),
        Load::Loading | Load::Failed(_) => None,
    }
}

/// What the actions reported; the fixture's own git watching can fail too.
fn error_titles(f: &GitFixture, cx: &mut TestAppContext) -> Vec<String> {
    f.ws.read_with(cx, |ws, _| {
        ws.error_history()
            .iter()
            .filter(|r| {
                r.dedup_key
                    .as_deref()
                    .is_some_and(|key| key.starts_with("github.pr_action"))
            })
            .map(|r| r.title.clone())
            .collect()
    })
}

#[gpui::test]
fn merge_asks_first_then_merges_once_with_the_chosen_method_at_the_shown_head(
    cx: &mut TestAppContext,
) {
    let (f, gh, id) = opened(cx, mergeable(7, "feat"));
    let reads = gh.count(GhOp::PrDetail);
    assert!(asks(&f, cx, |ws, window, cx| {
        ws.ask_pr_merge(id, MergeMethod::Merge, window, cx)
    }));
    cx.run_until_parked();
    assert_eq!(gh.count(GhOp::MergePr), 0, "nothing merges before OK");

    confirm(&f, cx);
    assert_eq!(
        merges(&gh),
        [GhCall::MergePr {
            repo: repo(),
            number: 7,
            method: MergeMethod::Merge,
            head_sha: "abc".into(),
        }]
    );
    assert_eq!(
        gh.count(GhOp::PrDetail),
        reads + 1,
        "the detail is read again"
    );
    assert_eq!(loaded_state(&f, cx), Some(PrState::Merged));
    assert!(error_titles(&f, cx).is_empty());
}

#[gpui::test]
fn a_moved_head_says_so_and_reloads_the_detail(cx: &mut TestAppContext) {
    let (f, gh, id) = opened(cx, mergeable(7, "feat"));
    // Pushed to after the detail was read.
    let mut moved = mergeable(7, "feat");
    moved.head_sha = "def".into();
    gh.set_pr_detail(&repo(), 7, moved);
    let reads = gh.count(GhOp::PrDetail);

    assert!(asks(&f, cx, |ws, window, cx| {
        ws.ask_pr_merge(id, MergeMethod::Squash, window, cx)
    }));
    confirm(&f, cx);
    assert_eq!(gh.count(GhOp::MergePr), 1);
    assert_eq!(
        error_titles(&f, cx),
        [crate::surface::strings::error::github_pr_head_moved(7)]
    );
    assert_eq!(
        gh.count(GhOp::PrDetail),
        reads + 1,
        "the detail is reloaded"
    );
    let Some((_, _, Load::Ready(loaded))) = shown(&f, cx) else {
        panic!("not read");
    };
    assert_eq!(loaded.head_sha, "def", "it shows the head that moved");
    assert_eq!(loaded.summary.state, PrState::Open);
}

#[gpui::test]
fn every_action_is_off_while_one_runs(cx: &mut TestAppContext) {
    let (f, gh, id) = opened(cx, mergeable(7, "feat"));
    f.ws.update(cx, |ws, cx| ws.rerun_pr_detail_checks(id, cx));
    let shown = view(&f, cx);
    let actions = shown
        .actions
        .clone()
        .expect("an open pull request has actions");
    assert_eq!(actions.merge.slot, ActionSlot::Waiting);
    assert_eq!(actions.close, ActionSlot::Waiting);
    let DetailBody::Ready(ready) = shown.body else {
        panic!("not read");
    };
    assert_eq!(ready.rerun, Some(ActionSlot::Running));

    // Asked again, or for another action, while it runs: nothing starts.
    f.ws.update(cx, |ws, cx| ws.rerun_pr_detail_checks(id, cx));
    assert!(!asks(&f, cx, |ws, window, cx| ws.ask_pr_close(id, window, cx)));
    assert!(!asks(&f, cx, |ws, window, cx| {
        ws.ask_pr_merge(id, MergeMethod::Merge, window, cx)
    }));
    cx.run_until_parked();
    assert_eq!(gh.count(GhOp::ClosePr) + gh.count(GhOp::MergePr), 0);
    let actions = view(&f, cx).actions.expect("still open");
    assert_eq!(actions.merge.slot, ActionSlot::Ready);
    assert_eq!(actions.close, ActionSlot::Ready);
}

#[gpui::test]
fn ready_close_and_rerun_each_ask_github_once(cx: &mut TestAppContext) {
    let mut draft = mergeable(7, "feat");
    draft.summary.state = PrState::Draft;
    let (f, gh, id) = opened(cx, draft);

    f.ws.update(cx, |ws, cx| ws.rerun_pr_detail_checks(id, cx));
    cx.run_until_parked();
    let reruns: Vec<u64> = gh
        .calls()
        .into_iter()
        .filter_map(|call| match call {
            GhCall::RerunFailed { run, .. } => Some(run),
            _ => None,
        })
        .collect();
    assert_eq!(reruns, [11, 12], "two failed jobs of run 11 are one rerun");

    assert!(view(&f, cx).actions.expect("a draft").ready.is_some());
    f.ws.update(cx, |ws, cx| ws.mark_pr_detail_ready(id, cx));
    cx.run_until_parked();
    assert_eq!(gh.count(GhOp::SetReady), 1);
    assert_eq!(loaded_state(&f, cx), Some(PrState::Open));
    assert_eq!(view(&f, cx).actions.expect("open").ready, None);

    assert!(asks(&f, cx, |ws, window, cx| ws.ask_pr_close(id, window, cx)));
    cx.run_until_parked();
    assert_eq!(gh.count(GhOp::ClosePr), 0, "nothing closes before OK");
    confirm(&f, cx);
    assert_eq!(gh.count(GhOp::ClosePr), 1);
    assert_eq!(loaded_state(&f, cx), Some(PrState::Closed));
    assert_eq!(view(&f, cx).actions, None, "a closed pull request has none");
    assert!(error_titles(&f, cx).is_empty());
}

#[gpui::test]
fn a_failed_action_reports_what_gh_said(cx: &mut TestAppContext) {
    let (f, gh, id) = opened(cx, mergeable(7, "feat"));
    gh.fail(
        GhOp::RerunFailed,
        GhFailure::Exit {
            code: Some(1),
            stderr: "run 11 cannot be rerun".into(),
        },
    );
    let reads = gh.count(GhOp::PrDetail);
    f.ws.update(cx, |ws, cx| ws.rerun_pr_detail_checks(id, cx));
    cx.run_until_parked();
    let report = f.ws.read_with(cx, |ws, _| {
        ws.error_history()
            .iter()
            .find(|r| r.dedup_key.as_deref() == Some("github.pr_action"))
            .cloned()
            .expect("the failure is reported")
    });
    assert_eq!(
        report.title,
        crate::surface::strings::error::github_pr_rerun_failed(7)
    );
    assert!(
        report.message.contains("run 11 cannot be rerun"),
        "{}",
        report.message
    );
    assert_eq!(gh.count(GhOp::PrDetail), reads, "the detail stays as shown");
    let DetailBody::Ready(ready) = view(&f, cx).body else {
        panic!("not read");
    };
    assert_eq!(
        ready.rerun,
        Some(ActionSlot::Ready),
        "and can be tried again"
    );
}

#[gpui::test]
fn an_answer_landing_after_its_detail_was_replaced_is_dropped(cx: &mut TestAppContext) {
    let (f, gh, id) = opened(cx, mergeable(7, "feat"));
    gh.set_pr_detail(&repo(), 8, mergeable(8, "other"));
    f.ws.update(cx, |ws, cx| ws.rerun_pr_detail_checks(id, cx));
    let reads = gh.count(GhOp::PrDetail);
    open(&f, item(8, "other"), cx);
    cx.run_until_parked();
    assert_eq!(gh.count(GhOp::RerunFailed), 2, "the rerun still ran");
    assert_eq!(
        gh.count(GhOp::PrDetail),
        reads + 1,
        "only #8's own read: #7's answer reloads nothing"
    );
    let Some((shown_id, 8, Load::Ready(_))) = shown(&f, cx) else {
        panic!("{:?}", shown(&f, cx));
    };
    assert_ne!(shown_id, id);
    let actions = view(&f, cx).actions.expect("#8 is open");
    assert_eq!(actions.merge.slot, ActionSlot::Ready, "#8 runs nothing");
}
