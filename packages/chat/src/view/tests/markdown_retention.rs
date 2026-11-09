//! A Markdown body keeps its parse while its row is scrolled out of the
//! virtualized list.
//!
//! A body over the vendored `TextView`'s synchronous-parse limit (4 KiB) starts
//! empty and fills in when its background parse lands. If the parse lives in
//! window keyed state, it is dropped the first frame the row is not drawn, so a
//! row scrolled back into view lays out nearly empty, the list clamps to its
//! end, and tail-follow re-engages under the reader's wheel.

use gpui::{AppContext as _, Entity, TestAppContext, point, px};

use super::{assistant_text_item, open_view};
use crate::view::AgentChatView;

/// Well past the 4 KiB synchronous-parse limit, and tall enough that the
/// wheel lands inside it rather than skipping over it. Hangul is three bytes a
/// character in UTF-8, so a Korean reply crosses the limit at ~1.4k characters.
fn long_body() -> String {
    (0..240)
        .map(|n| {
            format!("Paragraph {n}: 결론부터 말하면, 이 문단은 높이를 차지하기 위한 본문입니다.")
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Short turns below the long body, together taller than the viewport plus the
/// list's overdraw, so sitting at the tail leaves the long body undrawn.
fn filler_turns(count: usize) -> Vec<daruda_acp::ChatItem> {
    (0..count)
        .flat_map(|n| {
            [
                daruda_acp::ChatItem::UserText(format!("question {n}")),
                assistant_text_item(&format!("answer {n}")),
            ]
        })
        .collect()
}

fn update_in_window<R>(
    cx: &mut TestAppContext,
    window_handle: gpui::AnyWindowHandle,
    view: &Entity<AgentChatView>,
    f: impl FnOnce(&mut AgentChatView, &mut gpui::Window, &mut gpui::Context<AgentChatView>) -> R,
) -> R {
    cx.update_window(window_handle, |_, window, cx| {
        view.update(cx, |v, cx| f(v, window, cx))
    })
    .expect("the window is open")
}

/// Wheel up from the tail into a long body and fail the moment the list moves
/// the other way: resumed tail-follow, or a scroll top that slid back down
/// (a clamp to the end that leaves following off).
///
/// `drawn_before` first draws the body and lets its parse land, the way a
/// reader who already scrolled past it has; otherwise its first draw happens
/// under the wheel, as in a restored session scrolled up from the tail.
async fn wheel_up_into_a_long_body(cx: &mut TestAppContext, drawn_before: bool) {
    let body = long_body();
    assert!(
        body.len() > 4 * 1024,
        "the body must take the async parse path"
    );

    let (window_handle, view) = open_view(cx);
    let mut items = vec![
        daruda_acp::ChatItem::UserText("question".to_string()),
        assistant_text_item(&body),
    ];
    items.extend(filler_turns(30));
    update_in_window(cx, window_handle, &view, |v, window, cx| {
        v.seed_items_for_test(items, cx);
        v.set_all_folds(true, window, cx);
    });
    cx.run_until_parked();

    if drawn_before {
        view.update(cx, |v, cx| {
            v.list_state.scroll_to(gpui::ListOffset {
                item_ix: 0,
                offset_in_item: px(0.),
            });
            cx.notify();
        });
        cx.run_until_parked();
    }

    view.update(cx, |v, cx| v.scroll_to_bottom(cx));
    cx.run_until_parked();
    let mut vcx = gpui::VisualTestContext::from_window(window_handle, cx);
    vcx.run_until_parked();
    let (following, mut previous_top) = view.read_with(&vcx, |v, _| {
        (
            v.list_state.is_following_tail(),
            v.list_state.logical_scroll_top().item_ix,
        )
    });
    assert!(following, "the fixture starts at the tail");
    // The body's row must be off the drawn window here, or the scenario never
    // takes it out of view and proves nothing.
    assert!(
        previous_top > 4,
        "the tail must sit well below the long body (top row {previous_top})"
    );

    let viewport = vcx.update(|window, _| window.viewport_size());
    let over_list = point(viewport.width / 2., viewport.height / 3.);
    let mut reached_body = false;
    for step in 0..400 {
        vcx.simulate_event(gpui::ScrollEvent {
            position: over_list,
            delta: gpui::ScrollDelta::Pixels(point(px(0.), px(60.))),
            ..Default::default()
        });
        vcx.run_until_parked();
        let (following, top) = view.read_with(&vcx, |v, _| {
            (
                v.list_state.is_following_tail(),
                v.list_state.logical_scroll_top().item_ix,
            )
        });
        assert!(
            !following,
            "wheel step {step} scrolled up yet the list resumed tail-follow"
        );
        assert!(
            top <= previous_top,
            "wheel step {step} scrolled up yet the top row moved down ({previous_top} -> {top})"
        );
        previous_top = top;
        if top <= 2 {
            reached_body = true;
            break;
        }
    }
    assert!(reached_body, "the wheel never reached the long body");
}

#[gpui::test]
async fn wheel_back_into_a_long_body_does_not_resume_tail_follow(cx: &mut TestAppContext) {
    wheel_up_into_a_long_body(cx, true).await;
}

/// A first draw has no earlier height to collapse from, so this holds with or
/// without retention; it pins that path, which retention does not cover.
#[gpui::test]
async fn wheel_into_a_never_drawn_long_body_does_not_resume_tail_follow(cx: &mut TestAppContext) {
    wheel_up_into_a_long_body(cx, false).await;
}
