use daruda_github::PrSearchState;
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Instant;

use daruda_github::availability::{BackoffReason, GhAvailability};
use daruda_github::{CheckRollup, PrAudience, PrListItem, PrSearch, PrState, PrSummary, RepoId};

use super::labels;
use crate::github::PrListAnswer;
use crate::workspace::github_ops::pr_list::{HostList, PrListCache};

const GITHUB: &str = "github.com";
const CORP: &str = "git.corp.example";

fn gh_repo() -> RepoId {
    RepoId::new(GITHUB, "o", "r")
}

fn corp_repo() -> RepoId {
    RepoId::new(CORP, "o", "r")
}

fn item(repo: &RepoId, number: u64) -> PrListItem {
    PrListItem {
        repo: repo.clone(),
        pr: PrSummary {
            number,
            url: String::new(),
            title: String::new(),
            state: PrState::Open,
            head_branch: String::new(),
            cross_repo: false,
            checks: CheckRollup::None,
            failed: Vec::new(),
            review: None,
            updated_at: String::new(),
        },
        author: None,
        head_owner: None,
        base_branch: String::new(),
        comments: 0,
    }
}

fn answer(repo: &RepoId, numbers: &[u64], truncated: bool) -> PrListAnswer {
    PrListAnswer {
        search: Arc::new(PrSearch {
            items: numbers.iter().map(|n| item(repo, *n)).collect(),
            truncated,
        }),
        covered: Arc::new([repo.clone()].into()),
        at: Instant::now(),
        more: false,
        matches: 0,
    }
}

fn shown(hosts: Vec<(&str, HostList)>) -> PrListCache {
    PrListCache::shown_with(
        hosts
            .into_iter()
            .map(|(host, list)| {
                (
                    (
                        PrAudience::Mine,
                        PrSearchState::Open,
                        crate::github::Refine::default(),
                        host.to_owned(),
                    ),
                    list,
                )
            })
            .collect::<HashMap<_, _>>(),
    )
}

fn paused() -> GhAvailability {
    GhAvailability::Backoff {
        until: Instant::now(),
        reason: BackoffReason::Transient,
        resume: daruda_github::availability::Resume::Estimated,
    }
}

#[test]
fn a_tab_counts_what_is_known_and_marks_what_is_missing() {
    let both: BTreeSet<RepoId> = [gh_repo(), corp_repo()].into();
    let github_only: BTreeSet<RepoId> = [gh_repo()].into();
    let empty = BTreeSet::new();
    let ready = |_: &str| GhAvailability::Ready;
    let corp_logged_out = |host: &str| {
        if host == CORP {
            GhAvailability::LoggedOut
        } else {
            GhAvailability::Ready
        }
    };
    // (label, list, repos, availability, the Mine tab's label)
    type Availability = Box<dyn Fn(&str) -> GhAvailability>;
    let cases: Vec<(&str, PrListCache, &BTreeSet<RepoId>, Availability, &str)> = vec![
        (
            "every host answered",
            shown(vec![
                (GITHUB, HostList::Ready(answer(&gh_repo(), &[1, 2], false))),
                (CORP, HostList::Ready(answer(&corp_repo(), &[3], false))),
            ]),
            &both,
            Box::new(ready),
            "3",
        ),
        (
            "a logged-out host leaves the count a lower bound",
            shown(vec![(
                GITHUB,
                HostList::Ready(answer(&gh_repo(), &[1, 2], false)),
            )]),
            &both,
            Box::new(corp_logged_out),
            "2+",
        ),
        (
            "a failed search keeps its last answer as a lower bound",
            shown(vec![(
                GITHUB,
                HostList::Failed {
                    last: Some(answer(&gh_repo(), &[1, 2], false)),
                },
            )]),
            &github_only,
            Box::new(ready),
            "2+",
        ),
        (
            "a host still waiting on its answer",
            shown(vec![(
                CORP,
                HostList::Ready(answer(&corp_repo(), &[3], false)),
            )]),
            &both,
            Box::new(ready),
            "…",
        ),
        (
            "nothing could be read",
            shown(vec![]),
            &github_only,
            Box::new(|_: &str| GhAvailability::Missing),
            "Unavailable",
        ),
        (
            "a paused host with no answer yet",
            shown(vec![]),
            &github_only,
            Box::new(|_: &str| paused()),
            "Unavailable",
        ),
        (
            "a failed search with nothing before it",
            shown(vec![(GITHUB, HostList::Failed { last: None })]),
            &github_only,
            Box::new(ready),
            "Unavailable",
        ),
        (
            "a capped search",
            shown(vec![(
                GITHUB,
                HostList::Ready(answer(&gh_repo(), &[], true)),
            )]),
            &github_only,
            Box::new(ready),
            "0+",
        ),
        (
            "an empty scope",
            shown(vec![]),
            &empty,
            Box::new(ready),
            "0",
        ),
        (
            "an answer that did not cover the repository is still loading",
            shown(vec![(
                GITHUB,
                HostList::Ready(answer(&corp_repo(), &[3], false)),
            )]),
            &github_only,
            Box::new(ready),
            "…",
        ),
    ];
    for (label, list, repos, availability, expected) in cases {
        assert_eq!(
            labels(&list, PrSearchState::Open, repos, availability)[0],
            expected,
            "{label}"
        );
    }
}

#[test]
fn each_tab_reads_its_own_filter() {
    let repos: BTreeSet<RepoId> = [gh_repo()].into();
    let list = shown(vec![(GITHUB, HostList::Failed { last: None })]);
    assert_eq!(
        labels(&list, PrSearchState::Open, &repos, |_| {
            GhAvailability::Ready
        }),
        ["Unavailable", "…", "…"].map(str::to_owned),
    );
    assert_eq!(
        labels(&PrListCache::default(), PrSearchState::Open, &repos, |_| {
            GhAvailability::Unknown
        }),
        ["…", "…", "…"].map(str::to_owned),
        "a hidden page has nothing yet"
    );
}
