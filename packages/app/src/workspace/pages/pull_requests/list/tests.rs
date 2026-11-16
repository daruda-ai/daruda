use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use daruda_github::availability::{BackoffReason, GhAvailability};
use daruda_github::{CheckRollup, PrFilter, PrListItem, PrSearch, PrState, PrSummary, RepoId};
use daruda_store::project::LaneRef;

use super::{
    self as list, Blocked, GroupBody, LaneFacts, PageBody, PageInputs, PrPageList, Trouble,
};
use crate::github::PrListAnswer;
use crate::workspace::github_ops::pr_list::{HostList, PrListState};

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
        _aging: gpui::Task::ready(()),
    }
}

/// An answer holding `items` that asked about `covered`, arrived at `at`.
fn answer_at(items: Vec<PrListItem>, covered: &[RepoId], at: Instant) -> PrListAnswer {
    PrListAnswer {
        search: Arc::new(PrSearch {
            items,
            truncated: false,
        }),
        covered: Arc::new(covered.iter().cloned().collect()),
        at,
    }
}

fn answer(items: Vec<PrListItem>, covered: &[RepoId]) -> PrListAnswer {
    answer_at(items, covered, Instant::now())
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
    let ghe = RepoId::new("ghe.corp", "o", "r");
    let unasked = RepoId::new(HOST, "o", "unasked");
    let search = answer(vec![item(7, false), item(8, false)], &[repo(), ghe.clone()]);
    let ready = || Some(HostList::Ready(search.clone()));
    let failed = || {
        Some(HostList::Failed {
            last: Some(search.clone()),
        })
    };
    let backoff = |reason| GhAvailability::Backoff {
        until: std::time::Instant::now(),
        reason,
    };
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
            "another host logged out may not be GitHub at all",
            ghe.clone(),
            LoggedOut,
            None,
            Shape::Blocked(Blocked::UnconfirmedHost),
        ),
        (
            "nor may another host without gh",
            ghe.clone(),
            Missing,
            ready(),
            Shape::Blocked(Blocked::UnconfirmedHost),
        ),
        (
            "a forge that is not GitHub reads the same as an Enterprise host",
            RepoId::new("gitlab.com", "o", "r"),
            LoggedOut,
            None,
            Shape::Blocked(Blocked::UnconfirmedHost),
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
            failed(),
            Shape::Listed(vec![7, 8], Some(Trouble::SearchFailed)),
        ),
        (
            "a repository the answer did not ask about is still loading",
            unasked.clone(),
            Ready,
            ready(),
            Shape::Loading,
        ),
        (
            "one not asked about on a paused host says why",
            unasked.clone(),
            backoff(BackoffReason::RateLimited),
            ready(),
            Shape::Listed(Vec::new(), Some(Trouble::RateLimited)),
        ),
        (
            "a failed search has no rows for one it never asked about",
            unasked.clone(),
            Ready,
            failed(),
            Shape::Listed(Vec::new(), Some(Trouble::SearchFailed)),
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
    let repos = BTreeSet::from([repo(), other]);
    let state = shown(Some(HostList::Ready(PrListAnswer {
        search: Arc::new(PrSearch {
            items: vec![item(7, false), elsewhere],
            truncated: true,
        }),
        covered: Arc::new(repos.clone()),
        at: Instant::now(),
    })));
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
            hosts: times
                .into_iter()
                .map(|(host, at)| {
                    let list = HostList::Ready(answer_at(Vec::new(), &[], at));
                    (host.to_owned(), list)
                })
                .collect(),
            _aging: gpui::Task::ready(()),
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
