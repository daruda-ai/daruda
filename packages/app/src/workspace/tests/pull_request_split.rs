//! The Pull requests list beside an open detail: where the width holds both,
//! opening another pull request replaces only the detail, and the list keeps
//! its keyboard, selection and place; where it does not, the detail is shown
//! alone as before.

use daruda_github::PrAudience;
use gpui::{Modifiers, TestAppContext, VisualTestContext, point, px, size};

use super::pull_request_page::{item, linked_window, rows};
use crate::workspace::pages::Page;

fn shown_number(f: &super::GitFixture, vcx: &mut VisualTestContext) -> Option<u64> {
    f.ws.read_with(vcx, |ws, _| {
        ws.pages
            .pull_requests
            .detail
            .as_ref()
            .map(|detail| detail.item().pr.number)
    })
}

fn list_focused(f: &super::GitFixture, vcx: &mut VisualTestContext) -> bool {
    vcx.update(|window, cx| {
        f.ws.read(cx)
            .pages
            .pull_requests
            .focus_handle()
            .is_focused(window)
    })
}

#[gpui::test]
fn a_wide_page_opens_details_beside_the_list(cx: &mut TestAppContext) {
    let (f, gh, _) = linked_window(cx);
    gh.set_pr_list(PrAudience::Mine, (1..=40).map(item).collect());
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.cx.update(crate::bind_keys::register_static_bindings);
    vcx.simulate_resize(size(px(1400.), px(700.)));
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| ws.open_page(Page::PullRequests, window, cx));
    });
    vcx.run_until_parked();
    let order: Vec<String> = rows(&f, &mut vcx).into_iter().map(|(key, _)| key).collect();
    let scroll = f.ws.read_with(&vcx, |ws, _| {
        ws.workspace_page.state().unwrap().scroll.clone()
    });
    scroll.set_offset(point(px(0.), px(-150.)));
    f.ws.update(&mut vcx, |_, cx| cx.notify());
    vcx.run_until_parked();

    let selector =
        |key: &str| -> &'static str { Box::leak(format!("pr-row-{key}").into_boxed_str()) };
    let first = vcx.debug_bounds(selector(&order[5])).unwrap();
    vcx.simulate_click(first.center(), Modifiers::default());
    vcx.run_until_parked();
    let opened = shown_number(&f, &mut vcx);
    assert!(opened.is_some(), "the click opens a detail");
    let list = vcx.debug_bounds("pr-split-list").expect("the list stays");
    let detail = vcx
        .debug_bounds("pr-split-detail")
        .expect("the detail sits beside it");
    assert!(list.right() <= detail.left(), "{list:?} | {detail:?}");
    assert!(list_focused(&f, &mut vcx), "the list keeps the keyboard");
    // The first open re-flows the rows and may scroll to keep the opened one
    // in view (`the_opened_row_stays_in_view_when_the_list_reflows`); from
    // here on the column keeps its shape and its place exactly.
    let offset = scroll.offset();

    // Rows stack in the narrower column; take one still in view.
    let viewport = scroll.bounds();
    let second = order
        .iter()
        .filter(|key| !key.ends_with(&format!("#{}", opened.unwrap())))
        .filter_map(|key| vcx.debug_bounds(selector(key)))
        .find(|row| row.top() >= viewport.top() && row.bottom() <= viewport.bottom())
        .expect("another row is in view");
    vcx.simulate_click(second.center(), Modifiers::default());
    vcx.run_until_parked();
    let replaced = shown_number(&f, &mut vcx);
    assert_ne!(
        replaced, opened,
        "a second pull request replaces the detail"
    );
    assert_eq!(scroll.offset(), offset, "and the list stays where it was");
    assert!(list_focused(&f, &mut vcx));

    vcx.simulate_keystrokes("down enter");
    assert_ne!(
        shown_number(&f, &mut vcx),
        replaced,
        "arrowing on and pressing Enter opens the next one beside the list"
    );

    vcx.simulate_keystrokes("escape");
    assert_eq!(shown_number(&f, &mut vcx), None, "Escape closes the detail");
    assert!(vcx.debug_bounds("pr-split-detail").is_none());
    assert!(list_focused(&f, &mut vcx));
}

#[gpui::test]
fn a_narrow_page_shows_the_detail_alone(cx: &mut TestAppContext) {
    let (f, _gh, _) = linked_window(cx);
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.simulate_resize(size(px(900.), px(700.)));
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| {
            ws.open_page(Page::PullRequests, window, cx);
            ws.open_pr_detail(item(8), window, cx);
        });
    });
    vcx.run_until_parked();
    assert_eq!(shown_number(&f, &mut vcx), Some(8));
    assert!(vcx.debug_bounds("pr-split-list").is_none());
    assert!(
        vcx.debug_bounds("pr-list-body").is_none(),
        "the list is not drawn"
    );
    assert!(
        f.ws.read_with(&vcx, |ws, _| ws.pages.pull_requests.snapshot.is_none()),
        "nor staged"
    );
    assert!(!list_focused(&f, &mut vcx));
}

#[gpui::test]
fn dragging_the_divider_widens_the_list(cx: &mut TestAppContext) {
    let (f, _gh, _) = linked_window(cx);
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.simulate_resize(size(px(1400.), px(700.)));
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| {
            ws.open_page(Page::PullRequests, window, cx);
            ws.open_pr_detail(item(8), window, cx);
        });
    });
    vcx.run_until_parked();
    let before = vcx.debug_bounds("pr-split-list").unwrap();
    let divider = vcx
        .debug_bounds("pr-split-divider")
        .expect("the divider is drawn");
    let at = divider.center();
    vcx.simulate_mouse_down(at, gpui::MouseButton::Left, Modifiers::default());
    vcx.simulate_mouse_move(
        point(at.x + px(120.), at.y),
        gpui::MouseButton::Left,
        Modifiers::default(),
    );
    vcx.simulate_mouse_up(
        point(at.x + px(120.), at.y),
        gpui::MouseButton::Left,
        Modifiers::default(),
    );
    vcx.run_until_parked();
    let after = vcx.debug_bounds("pr-split-list").unwrap();
    assert_eq!(after.size.width, before.size.width + px(120.));
    assert!(
        !f.ws.read_with(&vcx, |ws, _| ws.pr_divider_dragging()),
        "the release ends the drag"
    );

    let toggle = vcx
        .debug_bounds("pr-list-toggle")
        .expect("the detail offers to hide the list");
    vcx.simulate_click(toggle.center(), Modifiers::default());
    vcx.run_until_parked();
    assert!(
        vcx.debug_bounds("pr-split-list").is_none(),
        "the detail is shown alone"
    );
    let toggle = vcx
        .debug_bounds("pr-list-toggle")
        .expect("and offers the list back");
    vcx.simulate_click(toggle.center(), Modifiers::default());
    vcx.run_until_parked();
    assert_eq!(
        vcx.debug_bounds("pr-split-list").unwrap().size.width,
        after.size.width,
        "the list comes back at the width it was given"
    );
}

/// Opening a detail beside the list re-flows the rows into the narrower
/// column, and closing it re-flows them back; the opened row stays in view
/// both times.
#[gpui::test]
fn the_opened_row_stays_in_view_when_the_list_reflows(cx: &mut TestAppContext) {
    let (f, gh, _) = linked_window(cx);
    gh.set_pr_list(PrAudience::Mine, (1..=40).map(item).collect());
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.cx.update(crate::bind_keys::register_static_bindings);
    vcx.simulate_resize(size(px(1400.), px(700.)));
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| ws.open_page(Page::PullRequests, window, cx));
    });
    vcx.run_until_parked();
    let order: Vec<String> = rows(&f, &mut vcx).into_iter().map(|(key, _)| key).collect();
    let scroll = f.ws.read_with(&vcx, |ws, _| {
        ws.workspace_page.state().unwrap().scroll.clone()
    });
    let selector =
        |key: &str| -> &'static str { Box::leak(format!("pr-row-{key}").into_boxed_str()) };
    let in_view = |vcx: &mut VisualTestContext, key: &str| {
        let viewport = scroll.bounds();
        let row = vcx.debug_bounds(selector(key)).unwrap();
        row.top() >= viewport.top() && row.bottom() <= viewport.bottom()
    };
    // The lowest row wholly in view in the table: compact rows are taller,
    // so at the same offset it would fall below the column's bottom.
    let low = order
        .iter()
        .filter(|key| in_view(&mut vcx, key))
        .last()
        .unwrap()
        .clone();
    let row = vcx.debug_bounds(selector(&low)).unwrap();
    vcx.simulate_click(row.center(), Modifiers::default());
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("pr-split-detail").is_some());
    assert!(
        in_view(&mut vcx, &low),
        "the opened row is still in view beside the detail"
    );

    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("pr-split-detail").is_none());
    assert!(in_view(&mut vcx, &low), "and in view again in the table");

    // Moving on beside the detail selects a row the table never showed at
    // its old place: closing brings that row into view instead.
    let row = vcx.debug_bounds(selector(&low)).unwrap();
    vcx.simulate_click(row.center(), Modifiers::default());
    vcx.run_until_parked();
    vcx.simulate_keystrokes("down down down down down down");
    let moved = f.ws.read_with(&vcx, |ws, _| ws.pr_cursor()).unwrap();
    let moved = order
        .iter()
        .find(|key| key.ends_with(&format!("#{}", moved.1)))
        .unwrap()
        .clone();
    assert_ne!(moved, low);
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(
        in_view(&mut vcx, &moved),
        "the row selected last is in view"
    );
}
