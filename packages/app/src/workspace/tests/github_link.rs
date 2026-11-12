//! The process-wide PR poller against scripted git and gh: one request per
//! repository however many windows ask, links follow the answers, and a
//! failure pauses polling quietly.

use std::time::Duration;

use daruda_github::availability::GhAuth;
use daruda_github::fake::{FakeGh, GhCall, GhFailure, GhOp};
use daruda_github::{CheckRollup, PrState, PrSummary, RepoId};
use daruda_store::project::LaneRef;
use gpui::TestAppContext;

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
        })
    });
    let id = f.ws.read_with(cx, |ws, _| ws.active_ref());
    f.ws.update(cx, |ws, cx| ws.refresh_git_status(id, cx));
    cx.run_until_parked();
    let (window, ws) = (f.window, f.ws.downgrade());
    cx.update(|cx| crate::window_registry::WindowRegistry::register(window.into(), ws, cx));
    (f, id)
}

fn create(f: &GitFixture, cx: &mut TestAppContext) {
    let request = crate::workspace::github_ops::create::CreatePrRequest {
        title: "Add it".into(),
        body: String::new(),
        base: None,
        draft: false,
    };
    let window = f.window.into();
    f.ws.update(cx, |ws, cx| {
        ws.create_pr(request, gpui::WeakEntity::new_invalid(), window, cx)
    });
    cx.run_until_parked();
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

#[gpui::test]
fn an_unpublished_branch_is_pushed_before_its_pr_opens(cx: &mut TestAppContext) {
    let gh = crate::test_support::fake_gh(cx);
    let (f, id) = lane_with(cx, false);
    create(&f, cx);
    let pushed = f.fake.calls().into_iter().any(|call| {
        call == crate::lane::git::GitCall::PushSetUpstream {
            repo: f.root.clone(),
            remote: "origin".into(),
            branch: "feat".into(),
        }
    });
    assert!(pushed, "{:?}", f.fake.calls());
    assert_eq!(gh.count(GhOp::CreatePr), 1);
    tick(cx);
    assert_eq!(
        linked(&f, id, cx),
        Some((1, PrState::Open)),
        "the poller finds it"
    );
}

#[gpui::test]
fn a_published_branch_with_nothing_new_is_not_pushed(cx: &mut TestAppContext) {
    let gh = crate::test_support::fake_gh(cx);
    let (f, _) = lane_with(cx, true);
    create(&f, cx);
    let pushes = f
        .fake
        .calls()
        .into_iter()
        .filter(|call| {
            matches!(
                call,
                crate::lane::git::GitCall::Push { .. }
                    | crate::lane::git::GitCall::PushSetUpstream { .. }
            )
        })
        .count();
    assert_eq!(pushes, 0);
    assert_eq!(gh.count(GhOp::CreatePr), 1);
}

#[gpui::test]
fn a_failed_push_opens_no_pr_and_says_so(cx: &mut TestAppContext) {
    let gh = crate::test_support::fake_gh(cx);
    let (f, _) = lane_with(cx, false);
    f.fake.fail(
        f.root.clone(),
        crate::lane::git::GitOp::PushSetUpstream,
        crate::lane::git::FakeFailure::Exit {
            code: Some(1),
            stderr: "rejected".into(),
        },
    );
    let before = toasts(&f, cx).len();
    create(&f, cx);
    assert_eq!(gh.count(GhOp::CreatePr), 0);
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
    let (f, _) = lane_with(cx, true);
    create(&f, cx);
    assert!(
        toasts(&f, cx).iter().any(|t| t.contains("already exists")),
        "gh's stderr reaches the report"
    );
}
