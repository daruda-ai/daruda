//! The pull request list across windows on different repositories: one
//! search answers the union, and a window says a repository is loading
//! until an answer that asked about it arrives.

use daruda_github::fake::{FakeGh, GhCall};
use daruda_github::{CheckRollup, PrFilter, PrListItem, PrState, PrSummary, RepoId};
use gpui::TestAppContext;

use crate::github::PrPoller;
use crate::lane::git::{FakeRepo, PushTarget};
use crate::workspace::pages::pull_requests::list::{GroupBody, PageBody};
use crate::workspace::tests::GitFixture;

fn repo(name: &str) -> RepoId {
    RepoId::new("github.com", "o", name)
}

fn item(name: &str, number: u64) -> PrListItem {
    PrListItem {
        repo: repo(name),
        pr: PrSummary {
            number,
            url: format!("https://github.com/o/{name}/pull/{number}"),
            title: format!("PR {number}"),
            state: PrState::Open,
            head_branch: "feat".into(),
            cross_repo: false,
            checks: CheckRollup::Passing,
            failed: Vec::new(),
            review: None,
            updated_at: "2026-01-01T00:00:00Z".into(),
        },
        author: Some("me".into()),
        head_owner: None,
        base_branch: "main".into(),
        comments: 0,
    }
}

fn pushing_to(repo: FakeRepo, name: &str) -> FakeRepo {
    repo.push_target(PushTarget {
        remote: "origin".into(),
        url: format!("git@github.com:o/{name}.git"),
        branch: "feat".into(),
        published: true,
        has_upstream: false,
    })
}

fn read_push_target(f: &GitFixture, cx: &mut TestAppContext) {
    let id = f.ws.read_with(cx, |ws, _| ws.active_ref());
    f.ws.update(cx, |ws, cx| ws.refresh_git_status(id, cx));
    cx.run_until_parked();
}

/// A registered window whose one lane pushes to `github.com/o/<name>`.
fn window(name: &str, cx: &mut TestAppContext) -> GitFixture {
    let f = GitFixture::open(cx, &daruda_config::Config::default(), |repo: FakeRepo| {
        pushing_to(repo, name)
    });
    read_push_target(&f, cx);
    let (window, ws) = (f.window, f.ws.downgrade());
    cx.update(|cx| crate::window_registry::WindowRegistry::register(window.into(), ws, cx));
    f
}

/// `gh` listing #7 in o/r and #8 in o/s as the user's.
fn gh(cx: &mut TestAppContext) -> FakeGh {
    let gh = crate::test_support::fake_gh(cx);
    gh.set_pr_list(PrFilter::Mine, vec![item("r", 7), item("s", 8)]);
    gh
}

fn tick(cx: &mut TestAppContext) {
    cx.update(PrPoller::tick);
    cx.run_until_parked();
}

/// Show the page's list without letting the search it starts answer.
fn show_unanswered(f: &GitFixture, cx: &mut TestAppContext) {
    f.ws.update(cx, |ws, cx| ws.show_pr_list(PrFilter::Mine, cx));
}

/// The pull request numbers `name`'s group lists; `None` while it loads.
fn group(f: &GitFixture, name: &str, cx: &mut TestAppContext) -> Option<Vec<u64>> {
    let list = f.ws.read_with(cx, |ws, cx| ws.pr_page_list(cx));
    let PageBody::Groups(groups) = list.body else {
        panic!("expected groups, got {:?}", list.body);
    };
    let group = groups
        .iter()
        .find(|group| group.repo == repo(name))
        .unwrap_or_else(|| panic!("no group for {name}"));
    match &group.body {
        GroupBody::Loading => None,
        GroupBody::Listed { rows, trouble, .. } => {
            assert_eq!(*trouble, None, "{name}");
            Some(rows.iter().map(|row| row.item.pr.number).collect())
        }
        GroupBody::Blocked(blocked) => panic!("{name}: {blocked:?}"),
    }
}

/// The repositories each search asked about, in order.
fn searched(gh: &FakeGh) -> Vec<Vec<RepoId>> {
    gh.calls()
        .into_iter()
        .filter_map(|call| match call {
            GhCall::SearchPrs { repos, .. } => Some(repos),
            _ => None,
        })
        .collect()
}

#[gpui::test]
fn a_late_window_loads_what_the_cached_answer_did_not_cover(cx: &mut TestAppContext) {
    let gh = gh(cx);
    let first = window("r", cx);
    let late = window("s", cx);
    tick(cx);
    show_unanswered(&first, cx);
    cx.run_until_parked();
    assert_eq!(group(&first, "r", cx), Some(vec![7]));
    assert_eq!(searched(&gh), vec![vec![repo("r")]]);

    show_unanswered(&late, cx);
    assert_eq!(
        group(&late, "s", cx),
        None,
        "the cached answer never asked about o/s"
    );
    cx.run_until_parked();
    assert_eq!(group(&late, "s", cx), Some(vec![8]));
    assert_eq!(group(&first, "r", cx), Some(vec![7]));
    assert_eq!(
        searched(&gh),
        vec![vec![repo("r")], vec![repo("r"), repo("s")]],
        "the uncovered repository is asked about at once, with the union"
    );
}

#[gpui::test]
fn an_answer_in_flight_does_not_speak_for_a_window_that_joined(cx: &mut TestAppContext) {
    let gh = gh(cx);
    let first = window("r", cx);
    let joined = window("s", cx);
    tick(cx);
    show_unanswered(&first, cx);
    show_unanswered(&joined, cx);
    cx.run_until_parked();
    assert_eq!(searched(&gh), vec![vec![repo("r")]], "one search running");
    assert_eq!(group(&first, "r", cx), Some(vec![7]));
    assert_eq!(
        group(&joined, "s", cx),
        None,
        "the search was sent before o/s was wanted"
    );

    tick(cx);
    assert_eq!(group(&joined, "s", cx), Some(vec![8]));
    assert_eq!(
        searched(&gh),
        vec![vec![repo("r")], vec![repo("r"), repo("s")]]
    );
}

#[gpui::test]
fn a_repository_gained_later_is_read_from_the_answer_already_held(cx: &mut TestAppContext) {
    let gh = gh(cx);
    let other = window("s", cx);
    let f = window("r", cx);
    tick(cx);
    show_unanswered(&other, cx);
    cx.run_until_parked();
    show_unanswered(&f, cx);
    cx.run_until_parked();
    assert_eq!(group(&f, "r", cx), Some(vec![7]));
    let before = searched(&gh);
    assert_eq!(before.last(), Some(&vec![repo("r"), repo("s")]));

    // The window's lane now pushes to o/s, which the union already asked.
    f.fake
        .add_repo(pushing_to(FakeRepo::at(&f.root).branch("main"), "s"));
    read_push_target(&f, cx);
    assert_eq!(
        group(&f, "s", cx),
        Some(vec![8]),
        "the rows the process holds show without waiting for the cadence"
    );
    tick(cx);
    assert_eq!(searched(&gh), before, "nothing new to ask");

    // A page shown again later is filled from the process's answer.
    f.ws.update(cx, |ws, cx| ws.hide_pr_list(cx));
    show_unanswered(&f, cx);
    assert_eq!(group(&f, "s", cx), Some(vec![8]));
    cx.run_until_parked();
    assert_eq!(searched(&gh), before, "the cache covers it");
}
