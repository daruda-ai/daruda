use std::collections::{BTreeSet, HashMap};
use std::time::Duration;

use daruda_github::availability::{BackoffReason, GhAvailability};
use daruda_github::fake::{FakeGh, GhCall, GhFailure, GhOp};
use daruda_github::{CheckRollup, PrFilter, PrListItem, PrSearch, PrState, PrSummary, RepoId};
use daruda_store::project::LaneRef;
use gpui::{Modifiers, TestAppContext, VisualTestContext};

use super::list::{
    self, Blocked, GroupBody, LaneFacts, PageBody, PageInputs, PrPageList, RowLaneSlot, Trouble,
};
use crate::github::PrPoller;
use crate::lane::git::{FakeRepo, PushTarget};
use crate::workspace::github_ops::pr_list::{HostList, PrListState};
use crate::workspace::pages::Page;
use crate::workspace::tests::GitFixture;

const HOST: &str = "github.com";

fn repo() -> RepoId {
    RepoId::new(HOST, "o", "r")
}

fn summary(number: u64, head: &str, cross_repo: bool) -> PrSummary {
    PrSummary {
        number,
        url: format!("https://github.com/o/r/pull/{number}"),
        title: format!("PR {number}"),
        state: PrState::Open,
        head_branch: head.into(),
        cross_repo,
        checks: CheckRollup::Passing,
        failed: Vec::new(),
        review: None,
        updated_at: "2026-01-01T00:00:00Z".into(),
    }
}

fn item(number: u64, cross_repo: bool) -> PrListItem {
    PrListItem {
        repo: repo(),
        pr: summary(number, &format!("head-{number}"), cross_repo),
        author: Some("me".into()),
        head_owner: None,
        base_branch: "main".into(),
        comments: 0,
    }
}

// ---- Which lane works on a pull request ----

fn lane(project: u64, id: u64, repo: Option<RepoId>, branch: &str, pr: Option<u64>) -> LaneFacts {
    LaneFacts {
        lane: LaneRef { project, lane: id },
        name: branch.into(),
        pr: pr.map(|number| (repo.clone().unwrap(), number)),
        repo,
        branch: Some(branch.into()),
    }
}

#[test]
fn a_pull_request_finds_the_lane_working_on_it() {
    let other = RepoId::new(HOST, "o", "other");
    let lanes = vec![
        // Project 1 pushes to o/r: its linked lane and a fork review lane.
        lane(1, 0, Some(repo()), "feat", Some(7)),
        lane(1, 1, Some(repo()), "pr-9", None),
        // Project 2 pushes elsewhere, with a review branch of the same name.
        lane(2, 0, Some(other.clone()), "pr-10", None),
        lane(2, 1, Some(other), "pr-11", None),
        lane(1, 2, Some(repo()), "pr-11", None),
    ];
    /// `(project, lane)` ids.
    type Ids = Option<(u64, u64)>;
    // (label, item, expected lane)
    let cases: Vec<(&str, PrListItem, Ids)> = vec![
        ("linked by its branch", item(7, false), Some((1, 0))),
        ("same repository, not linked", item(8, false), None),
        (
            "a same-repository PR is not matched by branch name",
            item(9, false),
            None,
        ),
        (
            "a fork PR on its review branch",
            item(9, true),
            Some((1, 1)),
        ),
        (
            "a review branch in a project that pushes elsewhere",
            item(10, true),
            None,
        ),
        (
            "the review branch in the PR's own project wins",
            item(11, true),
            Some((1, 2)),
        ),
    ];
    for (label, item, expected) in cases {
        let found = list::linked_lane(&item, &lanes).map(|lane| lane.lane);
        let expected = expected.map(|(project, lane)| LaneRef { project, lane });
        assert_eq!(found, expected, "{label}");
    }
}

// ---- What a repository's group shows ----

/// What a group body comes to, without its rows' details.
#[derive(Debug, PartialEq)]
enum Shape {
    Loading,
    Blocked(Blocked),
    Listed(Vec<u64>, Option<Trouble>),
}

fn shape(list: &PrPageList) -> Shape {
    let PageBody::Groups(groups) = &list.body else {
        panic!("expected groups, got {:?}", list.body);
    };
    match &groups.first().expect("every repository has a group").body {
        GroupBody::Loading => Shape::Loading,
        GroupBody::Blocked(blocked) => Shape::Blocked(*blocked),
        GroupBody::Listed { rows, trouble, .. } => Shape::Listed(
            rows.iter()
                .map(|row| row.key.rsplit('#').next().unwrap().parse().unwrap())
                .collect(),
            *trouble,
        ),
    }
}

fn shown(host_list: Option<HostList>) -> PrListState {
    PrListState::Shown {
        filter: PrFilter::Mine,
        hosts: host_list
            .map(|list| HashMap::from([(HOST.to_owned(), list)]))
            .unwrap_or_default(),
        answered: HashMap::new(),
    }
}

fn page_for(repo: RepoId, availability: GhAvailability, state: &PrListState) -> PrPageList {
    let repos = BTreeSet::from([repo]);
    let inputs = PageInputs {
        enabled: true,
        filter: PrFilter::Mine,
        repos: &repos,
        list: state,
        lanes: &[],
        creating: &BTreeSet::new(),
        now: chrono::Utc::now(),
        clock: std::time::Instant::now(),
    };
    list::page(&inputs, |_| availability)
}

#[test]
fn a_group_says_what_its_host_allows() {
    let search = PrSearch {
        items: vec![item(7, false), item(8, false)],
        truncated: false,
    };
    let ready = || Some(HostList::Ready(search.clone()));
    let backoff = |reason| GhAvailability::Backoff {
        until: std::time::Instant::now(),
        reason,
    };
    let ghe = RepoId::new("ghe.corp", "o", "r");
    use GhAvailability::*;
    // (label, repository, availability, host list, expected)
    let cases: Vec<(&str, RepoId, GhAvailability, Option<HostList>, Shape)> = vec![
        ("not answered yet", repo(), Unknown, None, Shape::Loading),
        (
            "listed",
            repo(),
            Ready,
            ready(),
            Shape::Listed(vec![7, 8], None),
        ),
        (
            "gh missing",
            repo(),
            Missing,
            ready(),
            Shape::Blocked(Blocked::Missing),
        ),
        (
            "logged out",
            repo(),
            LoggedOut,
            None,
            Shape::Blocked(Blocked::LoggedOut),
        ),
        (
            "another host logged out says so in its group",
            ghe.clone(),
            LoggedOut,
            None,
            Shape::Blocked(Blocked::LoggedOut),
        ),
        (
            "another host without gh says so too",
            ghe.clone(),
            Missing,
            ready(),
            Shape::Blocked(Blocked::Missing),
        ),
        (
            "another host gh knows",
            ghe.clone(),
            Ready,
            None,
            Shape::Loading,
        ),
        (
            "a rate limit keeps the list and says so",
            repo(),
            backoff(BackoffReason::RateLimited),
            ready(),
            Shape::Listed(vec![7, 8], Some(Trouble::RateLimited)),
        ),
        (
            "a paused host with nothing yet says why",
            repo(),
            backoff(BackoffReason::Transient),
            None,
            Shape::Listed(Vec::new(), Some(Trouble::Unreachable)),
        ),
        (
            "a failed search keeps the last list",
            repo(),
            Ready,
            Some(HostList::Failed {
                last: Some(search.clone()),
            }),
            Shape::Listed(vec![7, 8], Some(Trouble::SearchFailed)),
        ),
        (
            "a failed first search",
            repo(),
            Ready,
            Some(HostList::Failed { last: None }),
            Shape::Listed(Vec::new(), Some(Trouble::SearchFailed)),
        ),
    ];
    for (label, repo, availability, host_list, expected) in cases {
        let state = shown(host_list);
        assert_eq!(
            shape(&page_for(repo, availability, &state)),
            expected,
            "{label}"
        );
    }
}

#[test]
fn only_a_failed_search_offers_to_search_again() {
    let cases = [
        (Trouble::SearchFailed, true),
        (Trouble::RateLimited, false),
        (Trouble::Unreachable, false),
    ];
    for (trouble, retryable) in cases {
        assert_eq!(trouble.retryable(), retryable, "{trouble:?}");
    }
}

#[test]
fn a_group_lists_only_its_own_repository() {
    let other = RepoId::new(HOST, "o", "other");
    let mut elsewhere = item(9, false);
    elsewhere.repo = other.clone();
    let state = shown(Some(HostList::Ready(PrSearch {
        items: vec![item(7, false), elsewhere],
        truncated: true,
    })));
    let repos = BTreeSet::from([repo(), other]);
    let inputs = PageInputs {
        enabled: true,
        filter: PrFilter::Mine,
        repos: &repos,
        list: &state,
        lanes: &[],
        creating: &BTreeSet::new(),
        now: chrono::Utc::now(),
        clock: std::time::Instant::now(),
    };
    let PageBody::Groups(groups) = list::page(&inputs, |_| GhAvailability::Ready).body else {
        panic!("expected groups");
    };
    let keys: Vec<Vec<String>> = groups
        .iter()
        .map(|group| match &group.body {
            GroupBody::Listed {
                rows, truncated, ..
            } => {
                assert!(
                    *truncated,
                    "{}: the host's search was cut short",
                    group.label
                );
                rows.iter().map(|row| row.key.clone()).collect()
            }
            other => panic!("{}: {other:?}", group.label),
        })
        .collect();
    assert_eq!(
        keys,
        vec![
            vec!["github.com/o/other#9".to_owned()],
            vec!["github.com/o/r#7".to_owned()]
        ]
    );
}

#[test]
fn the_header_says_how_old_the_newest_answer_is() {
    let clock = std::time::Instant::now();
    let ghe = RepoId::new("ghe.corp", "o", "r");
    let repos = BTreeSet::from([repo(), ghe.clone()]);
    let ago = |seconds| clock - Duration::from_secs(seconds);
    /// (label, answer times per host, expected)
    type Case = (
        &'static str,
        Vec<(&'static str, std::time::Instant)>,
        Option<String>,
    );
    let cases: Vec<Case> = vec![
        ("nothing arrived yet", vec![], None),
        (
            "just now",
            vec![(HOST, ago(12))],
            Some("Updated just now".into()),
        ),
        (
            "the newest host counts",
            vec![(HOST, ago(600)), ("ghe.corp", ago(120))],
            Some("Updated 2m ago".into()),
        ),
        (
            "a host no repository is on does not",
            vec![(HOST, ago(7200)), ("other.host", ago(1))],
            Some("Updated 2h ago".into()),
        ),
    ];
    for (label, times, expected) in cases {
        let state = PrListState::Shown {
            filter: PrFilter::Mine,
            hosts: HashMap::new(),
            answered: times
                .into_iter()
                .map(|(host, at)| (host.to_owned(), at))
                .collect(),
        };
        let inputs = PageInputs {
            enabled: true,
            filter: PrFilter::Mine,
            repos: &repos,
            list: &state,
            lanes: &[],
            creating: &BTreeSet::new(),
            now: chrono::Utc::now(),
            clock,
        };
        let page = list::page(&inputs, |_| GhAvailability::Ready);
        assert_eq!(page.refreshed, expected, "{label}");
    }
}

#[test]
fn a_page_with_nothing_to_search_says_why() {
    let state = shown(None);
    let none = BTreeSet::new();
    let some = BTreeSet::from([repo()]);
    for (label, enabled, repos, expected) in [
        ("turned off", false, &some, PageBody::Off),
        ("no GitHub remote", true, &none, PageBody::NoRepos),
    ] {
        let inputs = PageInputs {
            enabled,
            filter: PrFilter::Mine,
            repos,
            list: &state,
            lanes: &[],
            creating: &BTreeSet::new(),
            now: chrono::Utc::now(),
            clock: std::time::Instant::now(),
        };
        assert_eq!(
            list::page(&inputs, |_| GhAvailability::Ready).body,
            expected,
            "{label}"
        );
    }
}

// ---- The page in a window ----

fn tick(cx: &mut TestAppContext) {
    cx.update(PrPoller::tick);
    cx.run_until_parked();
}

/// A window whose one lane pushes `feat` to o/r, linked to PR #7, with
/// #7 and #8 among the user's open pull requests.
fn linked_window(cx: &mut TestAppContext) -> (GitFixture, FakeGh, LaneRef) {
    let gh = crate::test_support::fake_gh(cx);
    gh.set_prs(&repo(), vec![summary(7, "feat", false)]);
    gh.set_pr_list(PrFilter::Mine, vec![item(7, false), item(8, false)]);
    gh.set_pr_list(PrFilter::ReviewRequested, vec![item(8, false)]);
    let f = GitFixture::open(cx, &daruda_config::Config::default(), |repo: FakeRepo| {
        repo.push_target(PushTarget {
            remote: "origin".into(),
            url: "git@github.com:o/r.git".into(),
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
    tick(cx);
    (f, gh, id)
}

fn show_page(f: &GitFixture, cx: &mut TestAppContext) {
    f.ws.update(cx, |ws, cx| ws.show_page(Page::PullRequests, cx));
    cx.run_until_parked();
}

fn searches(gh: &FakeGh) -> Vec<PrFilter> {
    gh.calls()
        .into_iter()
        .filter_map(|call| match call {
            GhCall::SearchPrs { filter, .. } => Some(filter),
            _ => None,
        })
        .collect()
}

fn linked(slot: &RowLaneSlot) -> Option<LaneRef> {
    match slot {
        RowLaneSlot::Linked(lane) => Some(lane.lane),
        RowLaneSlot::Creating | RowLaneSlot::Absent(_) => None,
    }
}

fn rows(f: &GitFixture, cx: &mut TestAppContext) -> Vec<(String, Option<LaneRef>)> {
    let list = f.ws.read_with(cx, |ws, cx| ws.pr_page_list(cx));
    let PageBody::Groups(groups) = list.body else {
        panic!("expected groups, got {:?}", list.body);
    };
    groups
        .iter()
        .flat_map(|group| {
            let GroupBody::Listed { rows, .. } = &group.body else {
                panic!("{}: {:?}", group.label, group.body);
            };
            rows.iter()
                .map(|row| (row.key.clone(), linked(&row.lane)))
                .collect::<Vec<_>>()
        })
        .collect()
}

#[gpui::test]
fn opening_the_page_lists_its_pull_requests_with_their_lanes(cx: &mut TestAppContext) {
    let (f, gh, id) = linked_window(cx);
    assert!(
        searches(&gh).is_empty(),
        "nothing is searched before the page opens"
    );
    show_page(&f, cx);
    assert_eq!(searches(&gh), vec![PrFilter::Mine]);
    assert_eq!(
        rows(&f, cx),
        vec![
            ("github.com/o/r#7".to_owned(), Some(id)),
            ("github.com/o/r#8".to_owned(), None)
        ]
    );
}

#[gpui::test]
fn clicking_a_rows_lane_goes_to_it(cx: &mut TestAppContext) {
    let (f, _gh, linked) = linked_window(cx);
    // Stand on another lane, so the click has somewhere to go.
    let other_dir = f.root.parent().unwrap().join("other");
    std::fs::create_dir_all(&other_dir).unwrap();
    let other = LaneRef {
        project: linked.project,
        lane: 77,
    };
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| {
            let mut lane = crate::lane::Lane::default_for_project(77, other_dir.clone());
            lane.name = Some("other".into());
            ws.project_for_mut(linked.project).unwrap().lanes.push(lane);
            ws.activate_lane(other, window, cx);
            ws.open_page(Page::PullRequests, window, cx);
        });
        window.refresh();
    });
    vcx.run_until_parked();
    assert_eq!(f.ws.read_with(&vcx, |ws, _| ws.active), other);

    let lane = vcx
        .debug_bounds("pr-lane-github.com/o/r#7")
        .expect("the linked row shows its lane");
    vcx.simulate_click(lane.center(), Modifiers::default());
    vcx.run_until_parked();
    f.ws.read_with(&vcx, |ws, _| {
        assert_eq!(ws.active, linked);
        assert_eq!(ws.active_page(), None, "the lane is what is shown now");
    });
}

#[gpui::test]
fn a_failed_search_keeps_its_rows_and_try_again_searches_now(cx: &mut TestAppContext) {
    let (f, gh, id) = linked_window(cx);
    show_page(&f, cx);
    gh.fail(
        GhOp::SearchPrs,
        GhFailure::Exit {
            code: Some(1),
            stderr: "HTTP 502".into(),
        },
    );
    f.ws.update(cx, |ws, cx| ws.refresh_pr_list(cx));
    cx.run_until_parked();
    let list = f.ws.read_with(cx, |ws, cx| ws.pr_page_list(cx));
    assert_eq!(
        shape(&list),
        Shape::Listed(vec![7, 8], Some(Trouble::SearchFailed)),
        "the last rows stay under the failure"
    );

    gh.clear_failure(GhOp::SearchPrs);
    gh.set_pr_list(PrFilter::Mine, vec![item(7, false)]);
    let searched = gh.count(GhOp::SearchPrs);
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.update(|window, _| window.refresh());
    vcx.run_until_parked();
    let retry = vcx
        .debug_bounds("pr-retry-github.com/o/r")
        .expect("the failed group offers Try again");
    vcx.simulate_click(retry.center(), Modifiers::default());
    vcx.run_until_parked();
    assert_eq!(gh.count(GhOp::SearchPrs), searched + 1);
    assert_eq!(
        rows(&f, &mut vcx),
        vec![("github.com/o/r#7".to_owned(), Some(id))]
    );
}

#[gpui::test]
fn switching_the_filter_searches_only_that_filter(cx: &mut TestAppContext) {
    let (f, gh, _) = linked_window(cx);
    show_page(&f, cx);
    f.ws.update(cx, |ws, cx| {
        ws.set_pr_page_filter(PrFilter::ReviewRequested, cx)
    });
    cx.run_until_parked();
    assert_eq!(
        searches(&gh),
        vec![PrFilter::Mine, PrFilter::ReviewRequested]
    );
    assert_eq!(rows(&f, cx), vec![("github.com/o/r#8".to_owned(), None)]);

    cx.executor().advance_clock(Duration::from_secs(120));
    tick(cx);
    assert_eq!(
        searches(&gh).last(),
        Some(&PrFilter::ReviewRequested),
        "the cadence asks the shown filter"
    );
    assert_eq!(
        searches(&gh)
            .iter()
            .filter(|f| **f == PrFilter::Mine)
            .count(),
        1,
        "the filter left is not asked again"
    );

    // Reopening shows the filter last chosen.
    f.ws.update(cx, |ws, cx| ws.close_page(cx));
    show_page(&f, cx);
    let filter = f.ws.read_with(cx, |ws, _| ws.pr_list().filter());
    assert_eq!(filter, Some(PrFilter::ReviewRequested));
}

#[gpui::test]
fn closing_the_page_stops_its_searches(cx: &mut TestAppContext) {
    let (f, gh, _) = linked_window(cx);
    show_page(&f, cx);
    assert_eq!(gh.count(GhOp::SearchPrs), 1);
    f.ws.update(cx, |ws, cx| ws.close_page(cx));
    cx.executor().advance_clock(Duration::from_secs(600));
    tick(cx);
    f.ws.update(cx, |ws, cx| ws.refresh_pr_list(cx));
    cx.run_until_parked();
    assert_eq!(
        gh.count(GhOp::SearchPrs),
        1,
        "nothing is searched while closed"
    );
    assert_eq!(f.ws.read_with(cx, |ws, _| ws.pr_list().filter()), None);

    // Another page replacing it stops them too.
    show_page(&f, cx);
    f.ws.update(cx, |ws, cx| ws.show_page(Page::Tasks, cx));
    assert_eq!(f.ws.read_with(cx, |ws, _| ws.pr_list().filter()), None);
}

#[gpui::test]
fn the_saved_state_never_holds_the_page(cx: &mut TestAppContext) {
    let (f, _gh, _) = linked_window(cx);
    for before in [None, Some(Page::Tasks)] {
        if let Some(page) = before {
            f.ws.update(cx, |ws, cx| ws.show_page(page, cx));
        }
        show_page(&f, cx);
        let saved =
            f.ws.update(cx, |ws, cx| ws.snapshot_for_disk(cx).0.active_page);
        assert_eq!(saved, None, "after {before:?}");
    }
}

#[gpui::test]
fn the_dock_counts_the_open_pull_requests_lanes_are_on(cx: &mut TestAppContext) {
    let (f, _gh, _) = linked_window(cx);
    assert_eq!(f.ws.read_with(cx, |ws, _| ws.open_lane_pr_count()), 1);
}
