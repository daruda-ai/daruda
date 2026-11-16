use std::sync::Arc;
use std::time::Duration;

use daruda_github::{CheckRollup, PrListItem, PrSearch, PrState, PrSummary};

use super::*;

const HOST: &str = "github.com";

fn repo(name: &str) -> RepoId {
    RepoId::new(HOST, "o", name)
}

fn item(name: &str, number: u64) -> PrListItem {
    PrListItem {
        repo: repo(name),
        pr: PrSummary {
            number,
            url: String::new(),
            title: String::new(),
            state: PrState::Open,
            head_branch: "feat".into(),
            cross_repo: false,
            checks: CheckRollup::None,
            failed: Vec::new(),
            review: None,
            updated_at: String::new(),
        },
        author: None,
        head_owner: None,
        base_branch: "main".into(),
        comments: 0,
    }
}

/// An answer holding `items` that asked about `covered`, arrived at `at`.
fn answer(items: Vec<PrListItem>, covered: &[&str], at: Instant) -> PrListAnswer {
    PrListAnswer {
        search: Arc::new(PrSearch {
            items,
            truncated: false,
        }),
        covered: Arc::new(covered.iter().map(|name| repo(name)).collect()),
        at,
    }
}

fn shown(filter: PrFilter, host: Option<HostList>) -> PrListState {
    let mut state = PrListState::default();
    state.show(filter);
    if let (PrListState::Shown { hosts, .. }, Some(list)) = (&mut state, host) {
        hosts.insert(HOST.to_owned(), list);
    }
    state
}

#[test]
fn updates_change_only_the_shown_filter() {
    let at = Instant::now();
    let earlier = at - Duration::from_secs(60);
    // Every window's repositories, kept whole: the page reads its own.
    let both = answer(vec![item("r", 1), item("other", 2)], &["r", "other"], at);
    let narrow = answer(vec![item("r", 1)], &["r"], at);
    let old = answer(vec![item("r", 1)], &["r", "other"], earlier);
    let again = PrListAnswer { at, ..old.clone() };
    let ready = |answer: &PrListAnswer| Some(HostList::Ready(answer.clone()));
    let failed_over = |answer: &PrListAnswer| HostList::Failed {
        last: Some(answer.clone()),
    };
    /// (label, state before, update, expected host list, expected changed)
    type Row<'a> = (&'static str, PrListState, PrListUpdate<'a>, HostList, bool);
    let fresh = PrListUpdate::Answer {
        host: HOST,
        filter: PrFilter::Mine,
        answer: &both,
    };
    let cached = PrListUpdate::Cached {
        host: HOST,
        filter: PrFilter::Mine,
        answer: &both,
    };
    let failed = PrListUpdate::Failed {
        host: HOST,
        filter: PrFilter::Mine,
    };
    let cases: Vec<Row> = vec![
        (
            "an answer is kept whole, with what it asked about",
            shown(PrFilter::Mine, None),
            fresh,
            HostList::Ready(both.clone()),
            true,
        ),
        (
            "the same answer at a new time moves only the time",
            shown(PrFilter::Mine, ready(&old)),
            PrListUpdate::Answer {
                host: HOST,
                filter: PrFilter::Mine,
                answer: &again,
            },
            HostList::Ready(again.clone()),
            true,
        ),
        (
            "the answer already held changes nothing",
            shown(PrFilter::Mine, ready(&both)),
            fresh,
            HostList::Ready(both.clone()),
            false,
        ),
        (
            "another filter's answer is ignored",
            shown(PrFilter::AllOpen, None),
            fresh,
            HostList::Loading,
            false,
        ),
        (
            "hidden ignores answers",
            PrListState::Hidden,
            fresh,
            HostList::Loading,
            false,
        ),
        (
            "a cached answer fills a loading host",
            shown(PrFilter::Mine, None),
            cached,
            HostList::Ready(both.clone()),
            true,
        ),
        (
            "a cached answer already held repaints nothing",
            shown(PrFilter::Mine, ready(&both)),
            cached,
            HostList::Ready(both.clone()),
            false,
        ),
        (
            "a cached answer that covers more replaces a narrower one",
            shown(PrFilter::Mine, ready(&narrow)),
            cached,
            HostList::Ready(both.clone()),
            true,
        ),
        (
            "a newer cached answer replaces an older one",
            shown(PrFilter::Mine, ready(&old)),
            cached,
            HostList::Ready(both.clone()),
            true,
        ),
        (
            "an older cached answer that covers no more is not taken",
            shown(PrFilter::Mine, ready(&both)),
            PrListUpdate::Cached {
                host: HOST,
                filter: PrFilter::Mine,
                answer: &old,
            },
            HostList::Ready(both.clone()),
            false,
        ),
        (
            "a cached answer does not hide a failure, but gives it rows",
            shown(PrFilter::Mine, Some(HostList::Failed { last: None })),
            cached,
            failed_over(&both),
            true,
        ),
        (
            "a failure over the cached answer stays as it is",
            shown(PrFilter::Mine, Some(failed_over(&both))),
            cached,
            failed_over(&both),
            false,
        ),
        (
            "another filter's cached answer is ignored",
            shown(PrFilter::AllOpen, None),
            cached,
            HostList::Loading,
            false,
        ),
        (
            "a failure keeps the last answer on show",
            shown(PrFilter::Mine, ready(&both)),
            failed,
            failed_over(&both),
            true,
        ),
        (
            "a repeated failure changes nothing",
            shown(PrFilter::Mine, Some(failed_over(&both))),
            failed,
            failed_over(&both),
            false,
        ),
        (
            "a failure before any answer has no rows",
            shown(PrFilter::Mine, None),
            failed,
            HostList::Failed { last: None },
            true,
        ),
        (
            "a blocked host drops its list",
            shown(PrFilter::Mine, ready(&both)),
            PrListUpdate::HostBlocked { host: HOST },
            HostList::Loading,
            true,
        ),
        (
            "switching the feature off drops every list",
            shown(PrFilter::Mine, ready(&both)),
            PrListUpdate::Disabled,
            HostList::Loading,
            true,
        ),
    ];
    for (label, mut state, update, expected, changed) in cases {
        assert_eq!(state.apply(update), changed, "{label}: changed");
        assert_eq!(state.host(HOST), &expected, "{label}");
    }
}

#[test]
fn an_answer_covers_only_what_it_asked_about() {
    let held = answer(vec![item("r", 1)], &["r"], Instant::now());
    assert_eq!(
        held.covering(&repo("r")).map(|search| search.items.len()),
        Some(1)
    );
    assert_eq!(
        held.covering(&repo("s")),
        None,
        "not asked is not the same as none found"
    );
}

#[test]
fn each_hosts_answer_time_follows_its_answers() {
    let first = Instant::now();
    let later = first + Duration::from_secs(60);
    let arrived = |at| answer(vec![item("r", 1)], &["r"], at);
    let (early, late) = (arrived(first), arrived(later));
    let at = |answer| PrListUpdate::Answer {
        host: HOST,
        filter: PrFilter::Mine,
        answer,
    };
    let mut state = shown(PrFilter::Mine, None);
    assert_eq!(state.answered_at(HOST), None, "nothing arrived yet");
    assert!(state.apply(at(&early)));
    assert_eq!(state.answered_at(HOST), Some(first));
    assert!(!state.apply(at(&early)), "nothing new");
    assert!(state.apply(at(&late)), "only the time moved");
    assert_eq!(state.answered_at(HOST), Some(later));

    let failed = PrListUpdate::Failed {
        host: HOST,
        filter: PrFilter::Mine,
    };
    state.apply(failed);
    assert_eq!(state.answered_at(HOST), Some(later), "a failure keeps it");
    state.apply(PrListUpdate::HostBlocked { host: HOST });
    assert_eq!(state.answered_at(HOST), None, "a blocked host drops it");
    state.apply(at(&late));
    assert!(state.show(PrFilter::AllOpen));
    assert_eq!(state.answered_at(HOST), None, "another filter starts over");
}

#[test]
fn show_and_hide_report_changes() {
    let mut state = PrListState::default();
    assert!(state.show(PrFilter::Mine));
    assert!(!state.show(PrFilter::Mine), "already shown");
    state.apply(PrListUpdate::Answer {
        host: HOST,
        filter: PrFilter::Mine,
        answer: &answer(vec![item("r", 1)], &["r"], Instant::now()),
    });
    assert!(state.show(PrFilter::AllOpen));
    assert_eq!(
        state.host(HOST),
        &HostList::Loading,
        "another filter starts over"
    );
    assert!(state.hide());
    assert!(!state.hide(), "already hidden");
    assert_eq!(state.filter(), None);
}
