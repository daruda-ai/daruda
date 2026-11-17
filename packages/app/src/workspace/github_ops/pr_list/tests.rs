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

/// No repaint task: these states are never drawn.
fn idle() -> Task<()> {
    Task::ready(())
}

/// The cache as one tab reads it.
struct Viewed {
    cache: PrListCache,
    filter: PrFilter,
}

impl Viewed {
    fn apply(&mut self, update: PrListUpdate<'_>) -> bool {
        self.cache.apply(update)
    }

    fn host(&self, host: &str) -> &HostList {
        self.cache.host(self.filter, host)
    }

    fn answered_at(&self, host: &str) -> Option<Instant> {
        self.cache.answered_at(self.filter, host)
    }
}

fn shown(filter: PrFilter, host: Option<HostList>) -> Viewed {
    let mut cache = PrListCache::default();
    cache.show(idle);
    if let (Some(shown), Some(list)) = (&mut cache.shown, host) {
        shown.hosts.insert((filter, HOST.to_owned()), list);
    }
    Viewed { cache, filter }
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
    type Row<'a> = (&'static str, Viewed, PrListUpdate<'a>, HostList, bool);
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
            "another filter fills counts without changing this tab",
            shown(PrFilter::AllOpen, None),
            fresh,
            HostList::Loading,
            true,
        ),
        (
            "hidden ignores answers",
            Viewed {
                cache: PrListCache::default(),
                filter: PrFilter::Mine,
            },
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
            "another filter fills its count from cache",
            shown(PrFilter::AllOpen, None),
            cached,
            HostList::Loading,
            true,
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
    state.filter = PrFilter::AllOpen;
    assert_eq!(state.answered_at(HOST), None, "another filter has its own");
}

#[test]
fn show_and_hide_report_changes() {
    let mut state = PrListCache::default();
    assert!(state.show(idle));
    assert!(!state.show(idle), "already shown");
    state.apply(PrListUpdate::Answer {
        host: HOST,
        filter: PrFilter::Mine,
        answer: &answer(vec![item("r", 1)], &["r"], Instant::now()),
    });
    assert_eq!(
        state.host(PrFilter::AllOpen, HOST),
        &HostList::Loading,
        "each tab holds its own answer"
    );
    assert!(matches!(
        state.host(PrFilter::Mine, HOST),
        HostList::Ready(_)
    ));
    assert!(state.hide());
    assert!(!state.hide(), "already hidden");
    assert!(!state.is_shown());
    assert_eq!(state.host(PrFilter::Mine, HOST), &HostList::Loading);
}

#[gpui::test]
fn a_shown_page_repaints_as_its_answer_ages(cx: &mut gpui::TestAppContext) {
    use std::cell::Cell;
    use std::rc::Rc;

    let f =
        crate::workspace::tests::GitFixture::open(cx, &daruda_config::Config::default(), |repo| {
            repo
        });
    let repaints = Rc::new(Cell::new(0));
    cx.update(|cx| {
        let repaints = repaints.clone();
        cx.observe(&f.ws, move |_, _| repaints.set(repaints.get() + 1))
            .detach();
    });
    // No answer arrives in an outage, so nothing else repaints the label.
    let aged = |cx: &mut gpui::TestAppContext| {
        cx.run_until_parked();
        repaints.set(0);
        cx.executor().advance_clock(AGE_REPAINT * 2);
        cx.run_until_parked();
        repaints.get()
    };
    // What the window's other timers repaint over the same span, once
    // the work its opening started has settled.
    aged(cx);
    let hidden = aged(cx);

    f.ws.update(cx, |ws, cx| ws.show_pr_list(cx));
    assert_eq!(aged(cx), hidden + 2, "once per interval while shown");

    f.ws.update(cx, |ws, cx| ws.hide_pr_list(cx));
    assert_eq!(aged(cx), hidden, "hiding the page ends it");
}
