use daruda_github::{CheckRollup, PrListItem, PrState, PrSummary};

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

fn search(items: Vec<PrListItem>) -> PrSearch {
    PrSearch {
        items,
        truncated: false,
    }
}

fn mine() -> BTreeSet<RepoId> {
    BTreeSet::from([repo("r")])
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
    let both = search(vec![item("r", 1), item("other", 2)]);
    let answer = search(vec![item("r", 1)]);
    let ready = || Some(HostList::Ready(answer.clone()));
    /// (label, state before, update, expected host list, expected changed)
    type Row<'a> = (&'static str, PrListState, PrListUpdate<'a>, HostList, bool);
    let fresh = PrListUpdate::Answer {
        host: HOST,
        filter: PrFilter::Mine,
        search: &both,
    };
    let cached = PrListUpdate::Cached {
        host: HOST,
        filter: PrFilter::Mine,
        search: &both,
    };
    let failed = PrListUpdate::Failed {
        host: HOST,
        filter: PrFilter::Mine,
    };
    let cases: Vec<Row> = vec![
        (
            "an answer is narrowed to this window's repositories",
            shown(PrFilter::Mine, None),
            fresh,
            HostList::Ready(answer.clone()),
            true,
        ),
        (
            "the same answer again changes nothing",
            shown(PrFilter::Mine, ready()),
            fresh,
            HostList::Ready(answer.clone()),
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
            HostList::Ready(answer.clone()),
            true,
        ),
        (
            "a cached answer does not hide a failure",
            shown(PrFilter::Mine, Some(HostList::Failed { last: None })),
            cached,
            HostList::Failed { last: None },
            false,
        ),
        (
            "a failure keeps the last answer on show",
            shown(PrFilter::Mine, ready()),
            failed,
            HostList::Failed {
                last: Some(answer.clone()),
            },
            true,
        ),
        (
            "a repeated failure changes nothing",
            shown(
                PrFilter::Mine,
                Some(HostList::Failed {
                    last: Some(answer.clone()),
                }),
            ),
            failed,
            HostList::Failed {
                last: Some(answer.clone()),
            },
            false,
        ),
        (
            "a blocked host drops its list",
            shown(PrFilter::Mine, ready()),
            PrListUpdate::HostBlocked { host: HOST },
            HostList::Loading,
            true,
        ),
        (
            "switching the feature off drops every list",
            shown(PrFilter::Mine, ready()),
            PrListUpdate::Disabled,
            HostList::Loading,
            true,
        ),
    ];
    for (label, mut state, update, expected, changed) in cases {
        assert_eq!(state.apply(update, &mine()), changed, "{label}: changed");
        assert_eq!(state.host(HOST), &expected, "{label}");
    }
}

#[test]
fn show_and_hide_report_changes() {
    let mut state = PrListState::default();
    assert!(state.show(PrFilter::Mine));
    assert!(!state.show(PrFilter::Mine), "already shown");
    state.apply(
        PrListUpdate::Answer {
            host: HOST,
            filter: PrFilter::Mine,
            search: &search(vec![item("r", 1)]),
        },
        &mine(),
    );
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
