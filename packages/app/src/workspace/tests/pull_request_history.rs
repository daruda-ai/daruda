//! Reading past the first page and past open pull requests: the state the
//! tabs list, later pages on request, and what replaces them.

use daruda_github::availability::{CredentialSource, GhAccount};
use daruda_github::{PrAudience, PrSearchState, PrState};
use gpui::{Modifiers, TestAppContext, VisualTestContext, px, size};

use super::pull_request_page::{item, linked_window, rows, show_page};
use crate::github::PrPoller;
use crate::workspace::pages::Page;

fn ended(number: u64, state: PrState) -> daruda_github::PrListItem {
    let mut item = item(number);
    item.pr.state = state;
    item
}

fn requested_states(gh: &daruda_github::fake::FakeGh) -> Vec<PrSearchState> {
    gh.calls()
        .into_iter()
        .filter_map(|call| match call {
            daruda_github::fake::GhCall::SearchPrsPage { request } => {
                Some(request.criteria().state)
            }
            _ => None,
        })
        .collect()
}

#[gpui::test]
fn choosing_a_state_lists_merged_and_closed_pull_requests(cx: &mut TestAppContext) {
    let (f, gh, _) = linked_window(cx);
    gh.set_pr_list(
        PrAudience::Mine,
        vec![
            item(7),
            ended(5, PrState::Merged),
            ended(4, PrState::Closed),
        ],
    );
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.simulate_resize(size(px(1400.), px(800.)));
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| ws.open_page(Page::PullRequests, window, cx));
    });
    vcx.run_until_parked();
    let numbers = |vcx: &mut VisualTestContext| -> Vec<String> {
        rows(&f, vcx).into_iter().map(|(key, _)| key).collect()
    };
    assert_eq!(numbers(&mut vcx), ["github.com/o/r#7"], "open by default");
    assert!(!requested_states(&gh).contains(&PrSearchState::Merged));

    let merged = vcx.debug_bounds("pr-state-1").expect("the state control");
    vcx.simulate_click(merged.center(), Modifiers::default());
    vcx.run_until_parked();
    assert_eq!(numbers(&mut vcx), ["github.com/o/r#5"]);
    assert!(requested_states(&gh).contains(&PrSearchState::Merged));

    let all = vcx.debug_bounds("pr-state-3").unwrap();
    vcx.simulate_click(all.center(), Modifiers::default());
    vcx.run_until_parked();
    assert_eq!(
        numbers(&mut vcx),
        ["github.com/o/r#7", "github.com/o/r#5", "github.com/o/r#4"],
        "every state, newest first"
    );
    f.ws.read_with(&vcx, |ws, _| {
        assert_eq!(ws.pr_page_state(), PrSearchState::All);
    });
}

#[gpui::test]
fn a_later_page_is_read_on_request_and_a_refresh_starts_over(cx: &mut TestAppContext) {
    let (f, gh, _) = linked_window(cx);
    gh.set_pr_list(PrAudience::Mine, (1..=60).map(item).collect());
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.simulate_resize(size(px(1400.), px(800.)));
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| ws.open_page(Page::PullRequests, window, cx));
    });
    vcx.run_until_parked();
    assert_eq!(rows(&f, &mut vcx).len(), 50, "one page at first");
    let scroll = f.ws.read_with(&vcx, |ws, _| {
        ws.workspace_page.state().unwrap().scroll.clone()
    });
    scroll.set_offset(gpui::point(px(0.), -scroll.max_offset().y));
    f.ws.update(&mut vcx, |_, cx| cx.notify());
    vcx.run_until_parked();

    let more = vcx
        .debug_bounds("pr-load-more")
        .expect("a later page is offered");
    vcx.simulate_click(more.center(), Modifiers::default());
    vcx.run_until_parked();
    let listed = rows(&f, &mut vcx);
    assert_eq!(listed.len(), 60, "the 51st and later are reachable");
    let keys: std::collections::BTreeSet<&str> =
        listed.iter().map(|(key, _)| key.as_str()).collect();
    assert_eq!(keys.len(), 60, "no pull request is listed twice");
    assert!(
        vcx.debug_bounds("pr-load-more").is_none(),
        "nothing further"
    );

    vcx.cx.update(PrPoller::tick);
    vcx.run_until_parked();
    assert_eq!(
        rows(&f, &mut vcx).len(),
        60,
        "the cadence keeps the longer list"
    );

    f.ws.update(&mut vcx, |ws, cx| ws.refresh_pr_list(cx));
    vcx.run_until_parked();
    assert_eq!(
        rows(&f, &mut vcx).len(),
        50,
        "a refresh reads the first page again rather than joining it to old pages"
    );
}

#[gpui::test]
fn another_account_drops_what_the_last_one_listed(cx: &mut TestAppContext) {
    let (f, gh, _) = linked_window(cx);
    let account = |login: &str| GhAccount {
        login: login.into(),
        source: CredentialSource::Keyring,
    };
    gh.set_account("github.com", account("first"));
    f.ws.update(cx, |ws, cx| ws.recheck_gh("github.com", cx));
    cx.update(PrPoller::tick);
    cx.run_until_parked();
    show_page(&f, cx);
    assert_eq!(rows(&f, cx).len(), 2);

    // The next list answers as the second account; until it arrives the
    // first account's rows must not be shown under it.
    gh.set_pr_list(PrAudience::Mine, vec![item(9)]);
    gh.set_account("github.com", account("second"));
    f.ws.update(cx, |ws, cx| ws.recheck_gh("github.com", cx));
    cx.update(PrPoller::tick);
    cx.run_until_parked();
    assert_eq!(
        rows(&f, cx),
        vec![("github.com/o/r#9".to_owned(), None)],
        "the list was read again as the new account"
    );
}
