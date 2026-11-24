//! Browsing the Pull requests list from the keyboard: the arrows select, Enter
//! opens, Back returns to the same row and place, and the search keeps its
//! own keys.

use daruda_github::PrAudience;
use gpui::{TestAppContext, VisualTestContext, px, size};

use super::pull_request_page::{item, linked_window, repo, rows};
use crate::workspace::pages::Page;

/// Test-only: `debug_bounds` takes a `'static` selector.
fn selector(name: String) -> &'static str {
    Box::leak(name.into_boxed_str())
}

fn number(key: &str) -> u64 {
    key.rsplit_once('#').unwrap().1.parse().unwrap()
}

#[gpui::test]
fn the_keyboard_browses_opens_and_returns_to_the_selected_row(cx: &mut TestAppContext) {
    let (f, gh, _) = linked_window(cx);
    gh.set_pr_list(PrAudience::Mine, (1..=60).map(item).collect());
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.cx.update(crate::bind_keys::register_static_bindings);
    vcx.simulate_resize(size(px(1280.), px(500.)));
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| ws.open_page(Page::PullRequests, window, cx));
    });
    vcx.run_until_parked();
    let order: Vec<String> = rows(&f, &mut vcx).into_iter().map(|(key, _)| key).collect();
    let cursor = |vcx: &mut VisualTestContext| {
        f.ws.read_with(vcx, |ws, _| ws.pr_cursor().map(|(_, number)| number))
    };
    let scroll = f.ws.read_with(&vcx, |ws, _| {
        ws.workspace_page.state().unwrap().scroll.clone()
    });
    assert!(
        vcx.update(|window, cx| f
            .ws
            .read(cx)
            .pages
            .pull_requests
            .focus_handle()
            .is_focused(window)),
        "opening the page gives the list the keyboard"
    );

    vcx.simulate_keystrokes("down");
    assert_eq!(
        cursor(&mut vcx),
        Some(number(&order[0])),
        "first Down selects the first row"
    );
    assert!(
        vcx.debug_bounds(selector(format!("pr-row-selected-{}", order[0])))
            .is_some()
    );
    vcx.simulate_keystrokes("up");
    assert_eq!(
        cursor(&mut vcx),
        Some(number(&order[0])),
        "Up stops at the first row"
    );

    for _ in 0..40 {
        vcx.simulate_keystrokes("down");
    }
    let chosen = &order[40];
    assert_eq!(cursor(&mut vcx), Some(number(chosen)));
    let viewport = scroll.bounds();
    let row = vcx
        .debug_bounds(selector(format!("pr-row-{chosen}")))
        .unwrap();
    assert!(
        row.top() >= viewport.top() && row.bottom() <= viewport.bottom(),
        "the selected row is scrolled into view: {row:?} in {viewport:?}"
    );
    let offset = scroll.offset();
    assert!(offset.y < px(0.), "reaching it scrolled the list");

    vcx.simulate_keystrokes("enter");
    f.ws.read_with(&vcx, |ws, _| {
        let detail = ws
            .pages
            .pull_requests
            .detail()
            .expect("Enter opens the detail");
        assert_eq!(
            detail.tab(),
            crate::workspace::pages::pull_requests::detail::PrTab::Description
        );
        assert_eq!(
            (detail.item().repo.clone(), detail.item().pr.number),
            (repo(), number(chosen))
        );
    });

    vcx.simulate_keystrokes("escape");
    f.ws.read_with(&vcx, |ws, _| {
        assert!(ws.pr_detail_id().is_none(), "Escape goes back to the list");
        assert_eq!(ws.active_page(), Some(Page::PullRequests));
    });
    assert!(
        vcx.update(|window, cx| f
            .ws
            .read(cx)
            .pages
            .pull_requests
            .focus_handle()
            .is_focused(window)),
        "Back gives the list the keyboard again"
    );
    assert_eq!(
        cursor(&mut vcx),
        Some(number(chosen)),
        "the opened row stays selected"
    );
    assert_eq!(scroll.offset(), offset, "the list comes back where it was");
    vcx.simulate_keystrokes("down");
    assert_eq!(
        cursor(&mut vcx),
        Some(number(&order[41])),
        "browsing resumes from it"
    );

    vcx.simulate_keystrokes("cmd-f");
    assert!(
        vcx.update(|window, cx| f.ws.read(cx).pr_search_focused(window, cx)),
        "the search shortcut moves the keyboard to the search"
    );
    vcx.simulate_keystrokes("down enter");
    assert_eq!(
        cursor(&mut vcx),
        Some(number(&order[41])),
        "the search field keeps its own keys"
    );
    assert!(f.ws.read_with(&vcx, |ws, _| ws.pr_detail_id().is_none()));
}

#[gpui::test]
fn focusing_search_and_escaping_keeps_the_same_detail_at_both_widths(cx: &mut TestAppContext) {
    let (f, _gh, _) = linked_window(cx);
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.cx.update(crate::bind_keys::register_static_bindings);
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| {
            ws.open_page(Page::PullRequests, window, cx);
            ws.open_pr_detail(item(8), window, cx);
        });
    });
    vcx.run_until_parked();
    let id = f.ws.read_with(&vcx, |ws, _| ws.pr_detail_id().unwrap());
    for (width, beside) in [(1400., true), (700., false)] {
        vcx.simulate_resize(size(px(width), px(700.)));
        vcx.run_until_parked();
        vcx.update(|window, cx| f.ws.update(cx, |ws, cx| ws.focus_pr_search(window, cx)));
        vcx.run_until_parked();
        assert!(vcx.update(|window, cx| f.ws.read(cx).pr_search_focused(window, cx)));
        f.ws.read_with(&vcx, |ws, _| {
            assert_eq!(ws.pr_detail_id(), Some(id));
            assert_eq!(ws.shown_page_detail_id().is_some(), beside);
            assert_eq!(ws.pr_cursor().map(|(_, n)| n), Some(8));
        });
        vcx.simulate_keystrokes("escape");
        f.ws.read_with(&vcx, |ws, _| {
            assert_eq!(ws.shown_page_detail_id(), Some(id.into()));
            assert_eq!(ws.pr_detail_id(), Some(id));
        });
        assert!(!vcx.update(|window, cx| f.ws.read(cx).pr_search_focused(window, cx)));
    }
}

/// Searching beside a detail, then narrowing the window until the two no
/// longer fit: the list stays in view over the detail, not under it.
#[gpui::test]
fn narrowing_while_searching_beside_a_detail_keeps_the_list(cx: &mut TestAppContext) {
    let (f, _gh, _) = linked_window(cx);
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.cx.update(crate::bind_keys::register_static_bindings);
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| {
            ws.open_page(Page::PullRequests, window, cx);
            ws.open_pr_detail(item(8), window, cx);
        });
    });
    vcx.simulate_resize(size(px(1400.), px(700.)));
    vcx.run_until_parked();
    vcx.update(|window, cx| f.ws.update(cx, |ws, cx| ws.focus_pr_search(window, cx)));
    vcx.run_until_parked();
    assert!(
        f.ws.read_with(&vcx, |ws, _| ws.shown_page_detail_id().is_some()),
        "both fit: the detail stays beside the search"
    );
    vcx.simulate_resize(size(px(700.), px(700.)));
    vcx.run_until_parked();
    f.ws.read_with(&vcx, |ws, _| {
        assert!(ws.shown_page_detail_id().is_none(), "the list is in view");
        assert!(ws.pr_detail_id().is_some(), "the detail waits behind it");
    });
}
