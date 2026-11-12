//! The process-wide PR poller against scripted git and gh: one request per
//! repository however many windows ask, links follow the answers, and a
//! failure pauses polling quietly.

use std::time::Duration;

use daruda_github::availability::GhAuth;
use daruda_github::fake::{FakeGh, GhCall, GhFailure, GhOp};
use daruda_github::{CheckRollup, PrState, PrSummary, RepoId};
use daruda_store::project::LaneRef;
use gpui::{AppContext as _, TestAppContext};

use super::GitFixture;
use crate::github::PrPoller;
use crate::lane::git::{FakeRepo, PushTarget};

fn repo() -> RepoId {
    RepoId {
        host: "github.com".into(),
        owner: "o".into(),
        name: "r".into(),
    }
}

fn pr(number: u64, state: PrState, updated: &str) -> PrSummary {
    PrSummary {
        number,
        url: format!("https://github.com/o/r/pull/{number}"),
        title: format!("PR {number}"),
        state,
        head_branch: "feat".into(),
        cross_repo: false,
        checks: CheckRollup::Passing,
        failed: Vec::new(),
        review: None,
        updated_at: updated.into(),
    }
}

/// A workspace whose one lane pushes `feat` to `github.com/o/r`, its push
/// target read and its window registered with the poller's registry.
fn lane_pushing_feat(cx: &mut TestAppContext) -> (GitFixture, LaneRef) {
    let f = GitFixture::open(cx, &daruda_config::Config::default(), |repo: FakeRepo| {
        repo.push_target(PushTarget {
            remote: "origin".into(),
            url: "git@github.com:O/R.git".into(),
            branch: "feat".into(),
            published: true,
            has_upstream: false,
        })
    });
    let id = f.ws.read_with(cx, |ws, _| ws.active_ref());
    f.ws.update(cx, |ws, cx| ws.refresh_git_status(id, cx));
    cx.run_until_parked();
    let (window, ws) = (f.window, f.ws.downgrade());
    cx.update(|cx| crate::window_registry::WindowRegistry::register(window.into(), ws, cx));
    (f, id)
}

fn tick(cx: &mut TestAppContext) {
    cx.update(PrPoller::tick);
    cx.run_until_parked();
}

fn linked(f: &GitFixture, id: LaneRef, cx: &mut TestAppContext) -> Option<(u64, PrState)> {
    f.ws.read_with(cx, |ws, _| {
        ws.lane_scoped
            .get(&id)
            .and_then(|state| state.pr.as_ref())
            .map(|link| (link.pr.number, link.pr.state))
    })
}

fn gh_with_open_pr(cx: &mut TestAppContext) -> FakeGh {
    let gh = crate::test_support::fake_gh(cx);
    gh.set_prs(&repo(), vec![pr(7, PrState::Open, "2026-01-01")]);
    gh
}

#[gpui::test]
fn two_windows_on_one_repository_cost_one_request(cx: &mut TestAppContext) {
    let gh = gh_with_open_pr(cx);
    let (first, first_id) = lane_pushing_feat(cx);
    let (second, second_id) = lane_pushing_feat(cx);
    tick(cx);
    assert_eq!(linked(&first, first_id, cx), Some((7, PrState::Open)));
    assert_eq!(linked(&second, second_id, cx), Some((7, PrState::Open)));
    let polls: Vec<GhCall> = gh
        .calls()
        .into_iter()
        .filter(|call| matches!(call, GhCall::PrsForBranches { .. }))
        .collect();
    assert_eq!(
        polls,
        vec![GhCall::PrsForBranches {
            repo: repo(),
            branches: vec!["feat".into()],
        }],
        "one request, the shared branch asked once"
    );
    assert_eq!(gh.count(GhOp::Auth), 1, "the host is checked once");
}

#[gpui::test]
fn a_later_answer_moves_the_link_to_merged(cx: &mut TestAppContext) {
    let gh = gh_with_open_pr(cx);
    let (f, id) = lane_pushing_feat(cx);
    tick(cx);
    assert_eq!(linked(&f, id, cx), Some((7, PrState::Open)));

    gh.set_prs(&repo(), vec![pr(7, PrState::Merged, "2026-01-02")]);
    tick(cx);
    assert_eq!(gh.count(GhOp::PrsForBranches), 1, "not due yet");
    cx.executor().advance_clock(Duration::from_secs(31));
    tick(cx);
    assert_eq!(linked(&f, id, cx), Some((7, PrState::Merged)));
}

#[gpui::test]
fn a_failed_request_pauses_the_repository_without_a_toast(cx: &mut TestAppContext) {
    let gh = gh_with_open_pr(cx);
    gh.fail(
        GhOp::PrsForBranches,
        GhFailure::Exit {
            code: Some(1),
            stderr: "error connecting to api.github.com".into(),
        },
    );
    let (f, id) = lane_pushing_feat(cx);
    let toasts = |cx: &mut TestAppContext| {
        f.ws.read_with(cx, |ws, cx| ws.toast_layer.read(cx).queue.len())
    };
    let before = toasts(cx);
    tick(cx);
    assert_eq!(gh.count(GhOp::PrsForBranches), 1);
    assert_eq!(linked(&f, id, cx), None);
    assert_eq!(
        toasts(cx),
        before,
        "polling failures are logged, not toasted"
    );

    cx.executor().advance_clock(Duration::from_secs(40));
    tick(cx);
    assert_eq!(gh.count(GhOp::PrsForBranches), 1, "still backing off");

    gh.clear_failure(GhOp::PrsForBranches);
    cx.executor().advance_clock(Duration::from_secs(30));
    tick(cx);
    assert_eq!(gh.count(GhOp::PrsForBranches), 2, "retried after a minute");
    assert_eq!(linked(&f, id, cx), Some((7, PrState::Open)));
}

#[gpui::test]
fn a_logged_out_host_is_not_polled(cx: &mut TestAppContext) {
    let gh = gh_with_open_pr(cx);
    gh.set_auth("github.com", GhAuth::LoggedOut);
    let (f, id) = lane_pushing_feat(cx);
    tick(cx);
    assert_eq!(gh.count(GhOp::PrsForBranches), 0);
    assert_eq!(linked(&f, id, cx), None);
    assert_eq!(gh.count(GhOp::Auth), 1);
}

#[gpui::test]
fn turning_the_feature_off_stops_polling_and_clears_links(cx: &mut TestAppContext) {
    let gh = gh_with_open_pr(cx);
    let (f, id) = lane_pushing_feat(cx);
    tick(cx);
    assert_eq!(linked(&f, id, cx), Some((7, PrState::Open)));

    cx.update(|cx| {
        let mut config = daruda_config::Config::default();
        config.github.enabled = false;
        cx.global_mut::<crate::settings_store::SettingsStore>()
            .set_user_for_testing(config);
    });
    cx.executor().advance_clock(Duration::from_secs(31));
    tick(cx);
    assert_eq!(linked(&f, id, cx), None);
    assert_eq!(gh.count(GhOp::PrsForBranches), 1, "no request while off");
}

// ---- Creating a pull request ----

fn lane_with(cx: &mut TestAppContext, published: bool) -> (GitFixture, LaneRef) {
    let f = GitFixture::open(cx, &daruda_config::Config::default(), |repo: FakeRepo| {
        repo.push_target(PushTarget {
            remote: "origin".into(),
            url: "git@github.com:o/r.git".into(),
            branch: "feat".into(),
            published,
            has_upstream: false,
        })
    });
    let id = f.ws.read_with(cx, |ws, _| ws.active_ref());
    f.ws.update(cx, |ws, cx| ws.refresh_git_status(id, cx));
    cx.run_until_parked();
    let (window, ws) = (f.window, f.ws.downgrade());
    cx.update(|cx| crate::window_registry::WindowRegistry::register(window.into(), ws, cx));
    (f, id)
}

fn request(base: Option<&str>) -> crate::workspace::github_ops::create::CreatePrRequest {
    crate::workspace::github_ops::create::CreatePrRequest {
        title: "Add it".into(),
        body: String::new(),
        base: base.map(str::to_owned),
        draft: false,
    }
}

/// A Create PR form for `lane`, submitted with `base`; the form is returned
/// so a test can read what it was told.
fn create_with(
    f: &GitFixture,
    lane: LaneRef,
    base: Option<&str>,
    cx: &mut TestAppContext,
) -> gpui::Entity<crate::workspace::github_ops::create_modal::CreatePrModal> {
    let window: gpui::AnyWindowHandle = f.window.into();
    let ws = f.ws.downgrade();
    let root = f.root.clone();
    let modal = cx
        .update_window(window, |_, window, cx| {
            let repo = crate::git_host::GitHost::global(cx).open(&root);
            cx.new(|cx| {
                crate::workspace::github_ops::create_modal::CreatePrModal::new_for_test(
                    ws, lane, repo, window, cx,
                )
            })
        })
        .unwrap();
    let weak = modal.downgrade();
    f.ws.update(cx, |ws, cx| {
        ws.create_pr(lane, request(base), weak, window, cx)
    });
    cx.run_until_parked();
    modal
}

fn create(f: &GitFixture, lane: LaneRef, cx: &mut TestAppContext) -> Option<String> {
    let modal = create_with(f, lane, None, cx);
    modal.read_with(cx, |modal, _| modal.failure().map(str::to_owned))
}

fn toasts(f: &GitFixture, cx: &mut TestAppContext) -> Vec<String> {
    f.ws.read_with(cx, |ws, cx| {
        ws.toast_layer
            .read(cx)
            .queue
            .iter()
            .map(|toast| format!("{:?}", toast.report))
            .collect()
    })
}

fn pushes(f: &GitFixture) -> Vec<crate::lane::git::GitCall> {
    f.fake
        .calls()
        .into_iter()
        .filter(|call| {
            matches!(
                call,
                crate::lane::git::GitCall::Push { .. } | crate::lane::git::GitCall::PushHead { .. }
            )
        })
        .collect()
}

#[gpui::test]
fn an_unpublished_branch_is_pushed_with_its_upstream_before_its_pr_opens(cx: &mut TestAppContext) {
    let gh = crate::test_support::fake_gh(cx);
    let (f, id) = lane_with(cx, false);
    assert_eq!(create(&f, id, cx), None);
    assert_eq!(
        pushes(&f),
        vec![crate::lane::git::GitCall::PushHead {
            repo: f.root.clone(),
            remote: "origin".into(),
            branch: "feat".into(),
            set_upstream: true,
        }]
    );
    assert_eq!(gh.count(GhOp::CreatePr), 1);
    tick(cx);
    assert_eq!(
        linked(&f, id, cx),
        Some((1, PrState::Open)),
        "the poller finds it"
    );
}

#[gpui::test]
fn a_published_branch_is_pushed_explicitly_without_touching_its_upstream(cx: &mut TestAppContext) {
    // A cached ahead count can miss commits; an explicit push to the same
    // branch is a no-op when there are none, so it always runs.
    let gh = crate::test_support::fake_gh(cx);
    let (f, id) = lane_with(cx, true);
    assert_eq!(create(&f, id, cx), None);
    assert_eq!(
        pushes(&f),
        vec![crate::lane::git::GitCall::PushHead {
            repo: f.root.clone(),
            remote: "origin".into(),
            branch: "feat".into(),
            set_upstream: false,
        }]
    );
    assert_eq!(gh.count(GhOp::CreatePr), 1);
}

#[gpui::test]
fn a_failed_push_opens_no_pr_and_the_form_says_so(cx: &mut TestAppContext) {
    let gh = crate::test_support::fake_gh(cx);
    let (f, id) = lane_with(cx, false);
    f.fake.fail(
        f.root.clone(),
        crate::lane::git::GitOp::PushHead,
        crate::lane::git::FakeFailure::Exit {
            code: Some(1),
            stderr: "rejected".into(),
        },
    );
    let before = toasts(&f, cx).len();
    let failure = create(&f, id, cx);
    assert_eq!(gh.count(GhOp::CreatePr), 0);
    assert!(failure.is_some_and(|m| m.contains("rejected")));
    assert_eq!(toasts(&f, cx).len(), before + 1);
}

#[gpui::test]
fn a_gh_failure_keeps_its_message(cx: &mut TestAppContext) {
    let gh = crate::test_support::fake_gh(cx);
    gh.fail(
        GhOp::CreatePr,
        GhFailure::Exit {
            code: Some(1),
            stderr: "a pull request for branch feat already exists".into(),
        },
    );
    let (f, id) = lane_with(cx, true);
    let failure = create(&f, id, cx);
    assert!(failure.is_some_and(|m| m.contains("already exists")));
    assert!(toasts(&f, cx).iter().any(|t| t.contains("already exists")));
}

#[gpui::test]
fn a_held_git_lock_fails_the_form_instead_of_hanging_it(cx: &mut TestAppContext) {
    let gh = crate::test_support::fake_gh(cx);
    let (f, id) = lane_with(cx, true);
    let window: gpui::AnyWindowHandle = f.window.into();
    let ws = f.ws.downgrade();
    let root = f.root.clone();
    let modal = cx
        .update_window(window, |_, window, cx| {
            let repo = crate::git_host::GitHost::global(cx).open(&root);
            cx.new(|cx| {
                crate::workspace::github_ops::create_modal::CreatePrModal::new_for_test(
                    ws, id, repo, window, cx,
                )
            })
        })
        .unwrap();
    let weak = modal.downgrade();
    // A Fetch holds the repository lock until its background run returns.
    f.ws.update(cx, |ws, cx| {
        ws.on_fetch(cx);
        ws.create_pr(id, request(None), weak, window, cx);
    });
    cx.run_until_parked();
    assert!(
        modal.read_with(cx, |modal, _| modal.failure().is_some()),
        "the form hears why"
    );
    assert_eq!(gh.count(GhOp::CreatePr), 0);
    assert!(pushes(&f).is_empty());
}

#[gpui::test]
fn a_branch_that_pushes_to_its_own_base_is_refused(cx: &mut TestAppContext) {
    let gh = crate::test_support::fake_gh(cx);
    let (f, id) = lane_with(cx, true);
    let modal = create_with(&f, id, Some("feat"), cx);
    let failure = modal.read_with(cx, |modal, _| modal.failure().map(str::to_owned));
    assert!(failure.is_some_and(|m| m.contains("base")));
    assert!(pushes(&f).is_empty(), "nothing is pushed onto the base");
    assert_eq!(gh.count(GhOp::CreatePr), 0);
}

#[gpui::test]
fn create_pr_uses_the_lane_it_is_given_not_the_one_shown(cx: &mut TestAppContext) {
    let gh = crate::test_support::fake_gh(cx);
    let (f, id) = lane_with(cx, false);
    let elsewhere = LaneRef {
        project: id.project,
        lane: id.lane + 1000,
    };
    // The form passes the lane it was opened for. Given a lane that is not
    // shown, and here no longer exists, nothing falls back to the active one.
    let modal = create_with(&f, elsewhere, None, cx);
    let failure = modal.read_with(cx, |modal, _| modal.failure().map(str::to_owned));
    assert_eq!(
        failure.as_deref(),
        Some(crate::surface::strings::error::github_pr_lane_gone().as_str())
    );
    assert_eq!(gh.count(GhOp::CreatePr), 0);
    assert!(pushes(&f).is_empty());
}

// ---- Sending a PR's problems to the lane's agent ----

fn failing_pr() -> PrSummary {
    let mut failing = pr(7, PrState::Open, "2026-01-01");
    failing.checks = CheckRollup::Failing { failed: 2 };
    failing.failed = vec![
        daruda_github::FailedCheck {
            workflow: None,
            name: "build".into(),
            details_url: Some("https://github.com/o/r/actions/runs/11/job/22".into()),
            job: Some(daruda_github::ActionsJob { run: 11, job: 22 }),
        },
        daruda_github::FailedCheck {
            workflow: None,
            name: "prow".into(),
            details_url: Some("https://prow.example/1".into()),
            job: None,
        },
    ];
    failing
}

fn linked_failing_lane(cx: &mut TestAppContext) -> (GitFixture, FakeGh) {
    let gh = crate::test_support::fake_gh(cx);
    gh.set_prs(&repo(), vec![failing_pr()]);
    gh.set_failed_log(
        daruda_github::ActionsJob { run: 11, job: 22 },
        "job\tstep\t2026-01-01T00:00:00.0000000Z ##[group]Run cargo test\n\
         job\tstep\t2026-01-01T00:00:01.0000000Z test parse ... \u{1b}[31mFAILED\u{1b}[0m\n\
         job\tstep\t2026-01-01T00:00:02.0000000Z ##[error]Process completed with exit code 101.",
    );
    let (f, _) = lane_pushing_feat(cx);
    tick(cx);
    (f, gh)
}

#[gpui::test]
fn failed_checks_land_in_the_agent_input_unsent(cx: &mut TestAppContext) {
    let (f, gh) = linked_failing_lane(cx);
    cx.update_window(f.window.into(), |_, window, cx| {
        f.ws.update(cx, |ws, cx| {
            ws.split_focused_pane_kind(
                super::NewPaneKind::AgentChat,
                super::SplitDirection::Horizontal,
                window,
                cx,
            );
            ws.send_failed_checks(window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(
        gh.count(GhOp::FailedLog),
        1,
        "only the Actions check has a log"
    );
    let input = f.ws.read_with(cx, |ws, cx| {
        ws.input_dock.input.read(cx).value().to_string()
    });
    assert!(input.contains("Pull request #7"), "{input}");
    assert!(
        input.contains("test parse ... FAILED"),
        "the log, cleaned: {input}"
    );
    assert!(!input.contains('\u{1b}'));
    assert!(input.contains("(no log: not a GitHub Actions check)"));
}

#[gpui::test]
fn without_an_agent_pane_nothing_is_fetched(cx: &mut TestAppContext) {
    let (f, gh) = linked_failing_lane(cx);
    let before = toasts(&f, cx).len();
    cx.update_window(f.window.into(), |_, window, cx| {
        f.ws.update(cx, |ws, cx| ws.send_failed_checks(window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(gh.count(GhOp::FailedLog), 0);
    assert_eq!(toasts(&f, cx).len(), before + 1, "the user hears why");
}

fn send_reviews_with_a_chat(f: &GitFixture, cx: &mut TestAppContext) -> String {
    cx.update_window(f.window.into(), |_, window, cx| {
        f.ws.update(cx, |ws, cx| {
            ws.split_focused_pane_kind(
                super::NewPaneKind::AgentChat,
                super::SplitDirection::Horizontal,
                window,
                cx,
            );
            ws.send_review_comments(window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    f.ws.read_with(cx, |ws, cx| {
        ws.input_dock.input.read(cx).value().to_string()
    })
}

#[gpui::test]
fn open_review_threads_land_in_the_agent_input(cx: &mut TestAppContext) {
    let (f, gh) = linked_failing_lane(cx);
    gh.set_review_threads(
        7,
        daruda_github::ReviewThreads {
            threads: vec![daruda_github::ReviewThread {
                path: "src/a.rs".into(),
                lines: Some((3, 5)),
                outdated: false,
                comments: vec![daruda_github::ReviewComment {
                    author: Some("rev".into()),
                    body: "Name this better.".into(),
                }],
                more_comments: false,
            }],
            truncated: false,
        },
    );
    let input = send_reviews_with_a_chat(&f, cx);
    assert!(
        input.contains("In src/a.rs:L3-L5:\n> rev: Name this better."),
        "{input}"
    );
    assert_eq!(gh.count(GhOp::ReviewThreads), 1);
}

#[gpui::test]
fn no_open_thread_sends_nothing_and_says_so(cx: &mut TestAppContext) {
    let (f, _gh) = linked_failing_lane(cx);
    let before = toasts(&f, cx);
    let input = send_reviews_with_a_chat(&f, cx);
    assert!(input.is_empty(), "nothing was inserted: {input}");
    let after = toasts(&f, cx);
    assert!(
        after.len() > before.len()
            && after
                .iter()
                .any(|t| t.contains("no unresolved review comments")),
        "{after:?}"
    );
}

// ---- Poller cadence, recovery and the feature switch ----

fn push_target(url: &str, branch: &str) -> PushTarget {
    PushTarget {
        remote: "origin".into(),
        url: url.into(),
        branch: branch.into(),
        published: true,
        has_upstream: false,
    }
}

/// A lane whose push target is `target`, read and registered.
fn lane_pushing(cx: &mut TestAppContext, target: PushTarget) -> (GitFixture, LaneRef) {
    let f = GitFixture::open(cx, &daruda_config::Config::default(), |repo: FakeRepo| {
        repo.push_target(target)
    });
    let id = f.ws.read_with(cx, |ws, _| ws.active_ref());
    f.ws.update(cx, |ws, cx| ws.refresh_git_status(id, cx));
    cx.run_until_parked();
    let (window, ws) = (f.window, f.ws.downgrade());
    cx.update(|cx| crate::window_registry::WindowRegistry::register(window.into(), ws, cx));
    (f, id)
}

fn set_enabled(cx: &mut TestAppContext, enabled: bool) {
    cx.update(|cx| {
        let mut config = daruda_config::Config::default();
        config.github.enabled = enabled;
        cx.global_mut::<crate::settings_store::SettingsStore>()
            .set_user_for_testing(config);
    });
}

#[gpui::test]
fn switching_branch_relinks_to_the_new_branchs_pr(cx: &mut TestAppContext) {
    let gh = gh_with_open_pr(cx);
    let mut other = pr(9, PrState::Draft, "2026-01-02");
    other.head_branch = "other".into();
    gh.set_prs(&repo(), vec![pr(7, PrState::Open, "2026-01-01"), other]);
    let (f, id) = lane_pushing_feat(cx);
    tick(cx);
    assert_eq!(linked(&f, id, cx), Some((7, PrState::Open)));

    f.fake.add_repo(
        FakeRepo::at(&f.root)
            .branch("other")
            .push_target(push_target("git@github.com:o/r.git", "other")),
    );
    f.ws.update(cx, |ws, cx| ws.refresh_git_status(id, cx));
    cx.run_until_parked();
    assert_eq!(
        linked(&f, id, cx),
        Some((9, PrState::Draft)),
        "the move nudged the poller; no wait for the cadence"
    );
}

#[gpui::test]
fn recheck_picks_up_a_login_made_meanwhile(cx: &mut TestAppContext) {
    let gh = gh_with_open_pr(cx);
    gh.set_auth("github.com", GhAuth::LoggedOut);
    let (f, id) = lane_pushing_feat(cx);
    tick(cx);
    assert_eq!(linked(&f, id, cx), None);

    gh.set_auth("github.com", GhAuth::Ready);
    cx.update(|cx| PrPoller::recheck("github.com", cx));
    cx.run_until_parked();
    assert_eq!(linked(&f, id, cx), Some((7, PrState::Open)));
}

#[gpui::test]
fn a_poll_that_finds_no_login_is_not_rechecked_at_once(cx: &mut TestAppContext) {
    let gh = gh_with_open_pr(cx);
    let (_f, _) = lane_pushing_feat(cx);
    tick(cx);
    assert_eq!(gh.count(GhOp::Auth), 1);
    // Long after the auth check, the login is revoked and a poll finds out.
    cx.executor().advance_clock(Duration::from_secs(6 * 60));
    gh.fail(
        GhOp::PrsForBranches,
        GhFailure::Exit {
            code: Some(4),
            stderr: "gh auth login".into(),
        },
    );
    tick(cx);
    let polls = gh.count(GhOp::PrsForBranches);
    tick(cx);
    tick(cx);
    assert_eq!(
        gh.count(GhOp::Auth),
        1,
        "the poll's answer counts as the check"
    );
    assert_eq!(
        gh.count(GhOp::PrsForBranches),
        polls,
        "a logged-out host is not polled"
    );
}

#[gpui::test]
fn moving_the_branch_ends_the_wait_after_create(cx: &mut TestAppContext) {
    let gh = crate::test_support::fake_gh(cx);
    let (f, id) = lane_with(cx, true);
    // The PR is made, but the poll that would link it cannot reach GitHub.
    gh.fail(
        GhOp::PrsForBranches,
        GhFailure::Exit {
            code: Some(1),
            stderr: "error connecting to api.github.com".into(),
        },
    );
    assert_eq!(create(&f, id, cx), None);
    let work = |cx: &mut TestAppContext| {
        f.ws.read_with(cx, |ws, _| ws.lane_scoped.get(&id).map(|s| s.pr_work))
    };
    assert_eq!(
        work(cx),
        Some(crate::workspace::lane_scoped::PrWork::AwaitingNewPr)
    );

    f.fake.add_repo(
        FakeRepo::at(&f.root)
            .branch("other")
            .push_target(push_target("git@github.com:o/r.git", "other")),
    );
    f.ws.update(cx, |ws, cx| ws.refresh_git_status(id, cx));
    cx.run_until_parked();
    assert_eq!(
        work(cx),
        Some(crate::workspace::lane_scoped::PrWork::Idle),
        "Create works on the new branch"
    );
}

#[gpui::test]
fn a_rate_limit_grows_the_pause_once_per_round(cx: &mut TestAppContext) {
    let gh = crate::test_support::fake_gh(cx);
    let _a = lane_pushing(cx, push_target("git@github.com:o/a.git", "feat"));
    let _b = lane_pushing(cx, push_target("git@github.com:o/b.git", "feat"));
    tick(cx);
    let setup = gh.count(GhOp::PrsForBranches);
    gh.fail(
        GhOp::PrsForBranches,
        GhFailure::Exit {
            code: Some(1),
            stderr: "API rate limit exceeded".into(),
        },
    );
    // Both repositories are due in the same round and both hit the limit.
    cx.executor().advance_clock(Duration::from_secs(31));
    tick(cx);
    assert_eq!(gh.count(GhOp::PrsForBranches), setup + 2);
    // After the 5-minute floor: both fail again, still a 5-minute pause.
    cx.executor().advance_clock(Duration::from_secs(5 * 60 + 1));
    tick(cx);
    assert_eq!(gh.count(GhOp::PrsForBranches), setup + 4);
    // A per-answer count would have paused 8 minutes and skipped this round.
    cx.executor().advance_clock(Duration::from_secs(5 * 60 + 1));
    tick(cx);
    assert_eq!(gh.count(GhOp::PrsForBranches), setup + 6);
}

#[gpui::test]
fn push_targets_are_read_only_while_the_feature_is_on(cx: &mut TestAppContext) {
    let _gh = gh_with_open_pr(cx);
    let (f, id) = lane_pushing_feat(cx);
    tick(cx);
    set_enabled(cx, false);
    tick(cx);
    let reads = f.fake.reads(crate::lane::git::GitOp::PushTarget, &f.root);
    f.ws.update(cx, |ws, cx| ws.refresh_git_status(id, cx));
    cx.run_until_parked();
    assert_eq!(
        f.fake.reads(crate::lane::git::GitOp::PushTarget, &f.root),
        reads,
        "no push-target processes while off"
    );

    set_enabled(cx, true);
    tick(cx);
    assert!(
        f.fake.reads(crate::lane::git::GitOp::PushTarget, &f.root) > reads,
        "turning it on reads them again"
    );
    assert_eq!(linked(&f, id, cx), Some((7, PrState::Open)));
}

#[gpui::test]
fn a_focused_shell_is_passed_over_for_the_agent_chat(cx: &mut TestAppContext) {
    let (f, gh) = linked_failing_lane(cx);
    cx.update_window(f.window.into(), |_, window, cx| {
        f.ws.update(cx, |ws, cx| {
            let shell = ws.active_runtime().focused_pane_id;
            ws.split_focused_pane_kind(
                super::NewPaneKind::AgentChat,
                super::SplitDirection::Horizontal,
                window,
                cx,
            );
            ws.set_focused_pane(shell, window, cx);
            ws.send_failed_checks(window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(gh.count(GhOp::FailedLog), 1);
    let (input, chat_focused) = f.ws.read_with(cx, |ws, cx| {
        let focused = ws.active_runtime().focused_pane_id;
        let chat = ws
            .active_runtime()
            .panes
            .iter()
            .any(|pane| pane.id == focused && pane.agent_chat_view().is_some());
        (ws.input_dock.input.read(cx).value().to_string(), chat)
    });
    assert!(chat_focused, "the prompt went to the chat, not the shell");
    assert!(input.contains("Pull request #7"), "{input}");
}

#[gpui::test]
fn a_failed_read_frees_the_buttons_and_says_so(cx: &mut TestAppContext) {
    let (f, gh) = linked_failing_lane(cx);
    gh.fail(
        GhOp::ReviewThreads,
        GhFailure::Exit {
            code: Some(1),
            stderr: "error connecting to api.github.com".into(),
        },
    );
    let before = toasts(&f, cx).len();
    let input = send_reviews_with_a_chat(&f, cx);
    assert!(input.is_empty(), "nothing was inserted: {input}");
    assert!(toasts(&f, cx).len() > before);
    let work = f.ws.read_with(cx, |ws, _| {
        let lane = ws.active_ref();
        ws.lane_scoped.get(&lane).map(|state| state.pr_work)
    });
    assert_eq!(
        work,
        Some(crate::workspace::lane_scoped::PrWork::Idle),
        "the send buttons work again"
    );
}
