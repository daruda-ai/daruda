use daruda_github::fake::{FakeGh, GhCall};
use daruda_github::{PrAudience, PrSearchCriteria, PrSearchState};
use gpui::{TestAppContext, VisualTestContext};
use std::time::Duration;

use super::*;
use crate::workspace::pages::Page;
use crate::workspace::tests::GitFixture;
use crate::workspace::tests::pull_request_page::linked_window;

/// The criteria of every list request sent so far.
fn sent(gh: &FakeGh) -> Vec<PrSearchCriteria> {
    gh.calls()
        .into_iter()
        .filter_map(|call| match call {
            GhCall::SearchPrsPage { request } => Some(request.criteria().clone()),
            _ => None,
        })
        .collect()
}

fn refined(gh: &FakeGh) -> Vec<PrSearchCriteria> {
    sent(gh)
        .into_iter()
        .filter(|criteria| {
            !criteria.terms.is_empty() || criteria.label.is_some() || criteria.author.is_some()
        })
        .collect()
}

fn open(cx: &mut TestAppContext) -> (GitFixture, FakeGh, VisualTestContext) {
    let (f, gh, _) = linked_window(cx);
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| {
            ws.open_page(Page::PullRequests, window, cx);
            ws.set_pr_search_mode(SearchMode::GitHub, window, cx);
        });
    });
    vcx.run_until_parked();
    (f, gh, vcx)
}

fn type_text(f: &GitFixture, vcx: &mut VisualTestContext, text: &str) {
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| {
            ws.pages
                .pull_requests
                .search
                .update(cx, |input, cx| input.set_value(text, window, cx));
        });
    });
    vcx.run_until_parked();
}

fn pause(vcx: &mut VisualTestContext) {
    vcx.executor()
        .advance_clock(SEARCH_DEBOUNCE + Duration::from_millis(1));
    vcx.run_until_parked();
}

#[gpui::test]
fn a_github_search_waits_for_typing_and_asks_for_the_shown_tab_alone(cx: &mut TestAppContext) {
    let (f, gh, mut vcx) = open(cx);
    let bare = sent(&gh).len();
    for text in ["f", "fi", "fix", "fix label:ui"] {
        type_text(&f, &mut vcx, text);
    }
    assert!(refined(&gh).is_empty(), "nothing is sent while typing");
    pause(&mut vcx);
    let refined = refined(&gh);
    assert_eq!(
        refined.len(),
        1,
        "one request for the paused text: {refined:?}"
    );
    assert_eq!(refined[0].terms, ["fix"]);
    assert_eq!(refined[0].label.as_deref(), Some("ui"));
    assert_eq!(refined[0].audience, PrAudience::Mine, "the shown tab");
    assert_eq!(
        sent(&gh).len(),
        bare + 1,
        "the tab badges are not searched again"
    );
    f.ws.read_with(&vcx, |ws, _| {
        let snap = ws.pages.pull_requests.snapshot.as_ref().unwrap();
        assert_eq!(snap.facets.len(), 1, "the facet is a chip");
        assert_eq!(snap.problem, None);
    });
}

#[gpui::test]
fn a_search_the_tab_cannot_take_is_not_sent_and_says_why(cx: &mut TestAppContext) {
    let (f, gh, mut vcx) = open(cx);
    type_text(&f, &mut vcx, "author:octo");
    pause(&mut vcx);
    assert!(refined(&gh).is_empty(), "author: beside Mine is not sent");
    f.ws.read_with(&vcx, |ws, _| {
        let snap = ws.pages.pull_requests.snapshot.as_ref().unwrap();
        assert_eq!(snap.problem, Some(RefineProblem::NeedsAllTab));
    });
    f.ws.update(&mut vcx, |ws, cx| {
        ws.set_pr_page_filter(PrAudience::All, cx)
    });
    vcx.run_until_parked();
    let refined = refined(&gh);
    assert_eq!(refined.len(), 1, "on the All tab it is");
    assert_eq!(refined[0].author.as_deref(), Some("octo"));
}

#[gpui::test]
fn removing_a_chip_rewrites_the_field_and_searches_at_once(cx: &mut TestAppContext) {
    let (f, gh, mut vcx) = open(cx);
    type_text(&f, &mut vcx, "fix label:ui draft:true");
    pause(&mut vcx);
    vcx.update(|window, cx| f.ws.update(cx, |ws, cx| ws.remove_pr_facet(0, window, cx)));
    vcx.run_until_parked();
    f.ws.read_with(&vcx, |ws, cx| {
        assert_eq!(
            ws.pages.pull_requests.search.read(cx).value(),
            "fix draft:true"
        );
    });
    let last = refined(&gh).pop().unwrap();
    assert_eq!(last.label, None);
    assert_eq!(last.draft, Some(true));
}

#[gpui::test]
fn reset_clears_the_search_and_keeps_the_tab_state_and_mode(cx: &mut TestAppContext) {
    let (f, _gh, mut vcx) = open(cx);
    f.ws.update(&mut vcx, |ws, cx| {
        ws.set_pr_page_filter(PrAudience::ReviewRequested, cx);
        ws.set_pr_page_state(PrSearchState::Merged, cx);
    });
    type_text(&f, &mut vcx, "fix label:ui");
    pause(&mut vcx);
    vcx.update(|window, cx| f.ws.update(cx, |ws, cx| ws.clear_pr_filters(window, cx)));
    vcx.run_until_parked();
    f.ws.read_with(&vcx, |ws, cx| {
        assert_eq!(ws.pages.pull_requests.search.read(cx).value(), "");
        assert!(
            ws.pr_page_refine().is_empty(),
            "applied at once, not after a pause"
        );
        assert_eq!(ws.pr_page_filter(), PrAudience::ReviewRequested);
        assert_eq!(ws.pr_page_state(), PrSearchState::Merged);
        assert_eq!(ws.pr_search_mode(), SearchMode::GitHub);
    });
}

#[gpui::test]
fn the_tab_state_and_mode_are_kept_with_the_window_but_not_the_text(cx: &mut TestAppContext) {
    let (f, _gh, mut vcx) = open(cx);
    f.ws.update(&mut vcx, |ws, cx| {
        ws.set_pr_page_filter(PrAudience::All, cx);
        ws.set_pr_page_state(PrSearchState::Closed, cx);
    });
    type_text(&f, &mut vcx, "fix");
    pause(&mut vcx);
    let record =
        f.ws.read_with(&vcx, |ws, _| ws.pages.pull_requests.layout_record());
    assert_eq!(
        record.audience,
        Some(daruda_store::project::PullRequestsAudience::All)
    );
    assert_eq!(
        record.state,
        Some(daruda_store::project::PullRequestsState::Closed)
    );
    assert!(record.search_github);
    let json = serde_json::to_string(&record).unwrap();
    assert!(!json.contains("fix"), "the typed text is not kept: {json}");

    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| {
            ws.pages.pull_requests.restore_layout(&Default::default());
            ws.pages.pull_requests.restore_layout(&record);
            ws.sync_pr_search_placeholder(window, cx);
        });
    });
    f.ws.read_with(&vcx, |ws, _| {
        assert_eq!(ws.pr_page_filter(), PrAudience::All);
        assert_eq!(ws.pr_page_state(), PrSearchState::Closed);
        assert_eq!(ws.pr_search_mode(), SearchMode::GitHub);
    });
}

fn titled(number: u64, title: &str) -> daruda_github::PrListItem {
    let mut item = crate::workspace::tests::pull_request_page::item(number);
    item.pr.title = title.into();
    item
}

fn listed(f: &GitFixture, vcx: &mut VisualTestContext) -> Vec<String> {
    crate::workspace::tests::pull_request_page::rows(f, vcx)
        .into_iter()
        .map(|(key, _)| key)
        .collect()
}

#[gpui::test]
fn a_github_search_finds_a_match_outside_the_loaded_rows(cx: &mut TestAppContext) {
    let (f, gh, _) = linked_window(cx);
    let mut items: Vec<_> = (11..=70).map(|n| titled(n, "routine")).collect();
    items.push(titled(3, "Needle in the haystack"));
    gh.set_pr_list(PrAudience::Mine, items);
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| ws.open_page(Page::PullRequests, window, cx));
    });
    vcx.run_until_parked();
    let loaded = listed(&f, &mut vcx);
    assert_eq!(loaded.len(), 50);
    assert!(
        !loaded.contains(&"github.com/o/r#3".to_owned()),
        "past the first page"
    );
    f.ws.read_with(&vcx, |ws, _| {
        assert_eq!(
            ws.pages.pull_requests.snapshot.as_ref().unwrap().list.found,
            Some(61)
        );
    });

    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| {
            ws.set_pr_search_mode(SearchMode::GitHub, window, cx)
        });
    });
    type_text(&f, &mut vcx, "needle");
    pause(&mut vcx);
    assert_eq!(listed(&f, &mut vcx), ["github.com/o/r#3"]);
    f.ws.read_with(&vcx, |ws, _| {
        let snap = ws.pages.pull_requests.snapshot.as_ref().unwrap();
        assert_eq!(
            snap.list.found,
            Some(1),
            "GitHub's own count, not the loaded rows"
        );
    });
}

#[gpui::test]
fn an_earlier_search_and_state_are_not_shown_again_from_old_rows(cx: &mut TestAppContext) {
    let (f, gh, mut vcx) = open(cx);
    let _ = gh;
    type_text(&f, &mut vcx, "first");
    pause(&mut vcx);
    type_text(&f, &mut vcx, "second");
    pause(&mut vcx);
    f.ws.read_with(&vcx, |ws, _| {
        let held = ws.pr_list().host_searched(
            PrAudience::Mine,
            PrSearchState::Open,
            &Refine::parse("first"),
            "github.com",
        );
        assert_eq!(
            held,
            &crate::workspace::github_ops::pr_list::HostList::Loading,
            "the window let go of the earlier search"
        );
    });

    f.ws.update(&mut vcx, |ws, cx| {
        ws.set_pr_page_state(PrSearchState::Merged, cx)
    });
    vcx.run_until_parked();
    f.ws.update(&mut vcx, |ws, cx| {
        ws.set_pr_page_state(PrSearchState::Open, cx);
        let held = ws.pr_list().host_searched(
            PrAudience::Mine,
            PrSearchState::Open,
            ws.pr_page_refine(),
            "github.com",
        );
        assert_eq!(
            held,
            &crate::workspace::github_ops::pr_list::HostList::Loading,
            "coming back waits on a fresh answer rather than showing old rows"
        );
    });
    vcx.run_until_parked();
    f.ws.read_with(&vcx, |ws, _| {
        let held = ws.pr_list().host_searched(
            PrAudience::Mine,
            PrSearchState::Open,
            ws.pr_page_refine(),
            "github.com",
        );
        assert!(held.answer().is_some(), "and the fresh answer arrives");
    });
}

#[gpui::test]
fn a_search_not_sent_offers_nothing_of_the_bare_tab(cx: &mut TestAppContext) {
    let (f, gh, mut vcx) = open(cx);
    gh.set_pr_list(PrAudience::Mine, (1..=60).map(|n| titled(n, "x")).collect());
    f.ws.update(&mut vcx, |ws, cx| ws.refresh_pr_list(cx));
    vcx.run_until_parked();
    f.ws.read_with(&vcx, |ws, _| {
        let snap = ws.pages.pull_requests.snapshot.as_ref().unwrap();
        assert_eq!(
            snap.list.more,
            crate::workspace::pages::pull_requests::list::More::Offered
        );
    });
    type_text(&f, &mut vcx, "author:octo");
    pause(&mut vcx);
    f.ws.read_with(&vcx, |ws, _| {
        let snap = ws.pages.pull_requests.snapshot.as_ref().unwrap();
        assert_eq!(snap.problem, Some(RefineProblem::NeedsAllTab));
        assert_eq!(
            snap.list.more,
            crate::workspace::pages::pull_requests::list::More::None
        );
        assert_eq!(snap.list.refreshed, None);
        assert_eq!(snap.list.sources.label(), None);
        assert_eq!(snap.list.found, None);
    });
}
