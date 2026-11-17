use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use daruda_github::availability::GhAvailability;
use daruda_github::{PrFilter, PrSearch, RepoId};

use super::{Activity, Sources};
use crate::github::PrListAnswer;
use crate::workspace::github_ops::pr_list::{HostList, PrListCache};
use crate::workspace::pages::pull_requests::list::{self, PageBody, PageInputs};

#[test]
fn progress_distinguishes_discovery_first_load_and_retained_answers() {
    let repo = RepoId::new("github.com", "o", "r");
    let repos = BTreeSet::from([repo]);
    let clock = Instant::now();
    let answer = PrListAnswer {
        search: Arc::new(PrSearch::default()),
        covered: Arc::new(repos.clone()),
        at: clock,
    };
    // (label, enabled, repositories discovered, discovery running, search running, cached answer, expected)
    for (label, enabled, known, discovering, refreshing, cached, expected) in [
        (
            "disabled during discovery",
            false,
            false,
            true,
            false,
            false,
            Activity::Idle,
        ),
        (
            "initial worktree discovery",
            true,
            false,
            true,
            false,
            false,
            Activity::Discovering,
        ),
        (
            "genuinely no repository",
            true,
            false,
            false,
            false,
            false,
            Activity::Idle,
        ),
        (
            "waiting for first answer",
            true,
            true,
            false,
            false,
            false,
            Activity::Waiting,
        ),
        (
            "search running without an answer",
            true,
            true,
            false,
            true,
            false,
            Activity::Loading,
        ),
        (
            "refresh retains an empty successful answer",
            true,
            true,
            false,
            true,
            true,
            Activity::Refreshing,
        ),
        (
            "new repositories with retained answers",
            true,
            true,
            true,
            false,
            true,
            Activity::Discovering,
        ),
        (
            "refresh settled",
            true,
            true,
            false,
            false,
            true,
            Activity::Idle,
        ),
    ] {
        let cache = PrListCache::shown_with(if cached {
            HashMap::from([(
                (PrFilter::Mine, "github.com".into()),
                HostList::Ready(answer.clone()),
            )])
        } else {
            HashMap::new()
        });
        let empty = BTreeSet::new();
        let inputs = PageInputs {
            enabled,
            discovering,
            refreshing,
            filter: PrFilter::Mine,
            repos: if known { &repos } else { &empty },
            list: &cache,
            lanes: &[],
            creating: &BTreeSet::new(),
            now: chrono::Utc::now(),
            clock,
        };
        let page = list::page(&inputs, |_| GhAvailability::Ready);
        assert_eq!(page.activity, expected, "{label}");
        if !known && enabled {
            assert_eq!(
                page.body,
                if discovering {
                    PageBody::Discovering
                } else {
                    PageBody::NoRepos
                },
                "{label}"
            );
        }
    }
}

#[test]
fn partial_sources_count_only_covered_visible_answers_and_keep_their_ages() {
    let clock = Instant::now();
    let ready = RepoId::new("github.com", "o", "ready");
    let pending = RepoId::new("github.com", "o", "new");
    let stale = RepoId::new("ghe.corp", "o", "stale");
    let blocked = RepoId::new("other.corp", "o", "blocked");
    let repos = BTreeSet::from([ready.clone(), pending, stale.clone(), blocked.clone()]);
    let answer = |repo, seconds| PrListAnswer {
        search: Arc::new(PrSearch::default()),
        covered: Arc::new(BTreeSet::from([repo])),
        at: clock - Duration::from_secs(seconds),
    };
    let cache = PrListCache::shown_with(HashMap::from([
        (
            (PrFilter::Mine, "github.com".into()),
            HostList::Ready(answer(ready, 60)),
        ),
        (
            (PrFilter::Mine, "ghe.corp".into()),
            HostList::Failed {
                last: Some(answer(stale, 3600)),
            },
        ),
        // A blocked host cannot contribute even when an old answer exists.
        (
            (PrFilter::Mine, "other.corp".into()),
            HostList::Ready(answer(blocked, 0)),
        ),
    ]));
    let inputs = PageInputs {
        enabled: true,
        discovering: false,
        refreshing: false,
        filter: PrFilter::Mine,
        repos: &repos,
        list: &cache,
        lanes: &[],
        creating: &BTreeSet::new(),
        now: chrono::Utc::now(),
        clock,
    };
    let page = list::page(&inputs, |host| {
        if host == "other.corp" {
            GhAvailability::LoggedOut
        } else {
            GhAvailability::Ready
        }
    });
    assert_eq!(
        page.sources,
        Sources {
            loaded: 2,
            total: 4,
            unavailable: 2
        }
    );
    assert_eq!(page.activity, Activity::Waiting);
    assert!(
        !page.activity.busy(),
        "waiting for an uncovered repository allows refresh"
    );
    let PageBody::Groups(groups) = page.body else {
        panic!("expected groups");
    };
    let age = |name: &str| {
        groups
            .iter()
            .find(|group| group.repo.name() == name)
            .unwrap()
            .refreshed
            .clone()
    };
    assert_eq!(age("ready"), Some("Updated 1m ago".into()));
    assert_eq!(age("stale"), Some("Updated 1h ago".into()));
    assert_eq!(age("new"), None);
    assert_eq!(age("blocked"), None);
}
