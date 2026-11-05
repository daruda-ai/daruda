//! Long prompts: open while their turn runs, folded from its settle edge.
use super::*;
use crate::fold::FoldKey;
use daruda_acp::{AcpEvent, ChatItem};

#[gpui::test]
fn a_long_prompt_folds_at_the_settle_and_the_next_prompt_stays_open(cx: &mut gpui::TestAppContext) {
    let window = make_test_view(cx);
    window
        .update(cx, |view, _window, cx| {
            let long = "log line\n".repeat(40);
            view.items = vec![ChatItem::UserText(long), assistant_text_item("reading")];
            view.set_turn_in_flight();
            let expanded = |view: &super::super::AgentChatView, ix| {
                let key = FoldKey::UserPrompt(ix);
                view.fold.is_expanded(
                    &key,
                    crate::agent_chat_helpers::fold_context(
                        &key,
                        &view.items,
                        &view.live_units,
                        view.filter_matches.standalone_calls(),
                    ),
                )
            };
            assert!(expanded(view, 0), "open while the agent works");

            view.apply_event(
                AcpEvent::TurnEnded {
                    stop_reason: "EndTurn".into(),
                    completed_normally: true,
                    usage: None,
                },
                "",
                false,
                cx,
            );
            assert!(!expanded(view, 0), "folded once its turn settled");

            // The next prompt lands after that settle, as a pumped or typed send
            // does; the settle already happened, so it must not fold with it.
            view.items.push(ChatItem::UserText("more log\n".repeat(40)));
            let next = view.items.len() - 1;
            assert!(
                expanded(view, next),
                "a prompt sent after the settle is open"
            );
        })
        .unwrap();
}

#[test]
fn a_preview_keeps_the_opening_lines_and_caps_one_huge_line() {
    use crate::render::prompt_preview_for_test as preview;
    let many = (1..=20)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    let shown = preview(&many);
    assert!(shown.starts_with("line 1\nline 2"));
    assert!(!shown.contains("line 7"));
    assert!(shown.ends_with('…'));

    let one = "x".repeat(5_000);
    assert_eq!(
        preview(&one).chars().count(),
        crate::PROMPT_PREVIEW_CHARS + 1
    );
}

/// The user's path through a folded prompt: read the preview, open it, fold it
/// back — each click changing the row's height.
#[gpui::test]
fn the_toggle_opens_and_refolds_a_settled_prompt(cx: &mut gpui::TestAppContext) {
    let window = make_test_view(cx);
    window
        .update(cx, |view, _window, cx| {
            view.items = vec![
                ChatItem::UserText("pasted log line\n".repeat(40)),
                assistant_text_item("read it"),
            ];
            view.settle_prompts();
            view.rebuild_rows();
            cx.notify();
        })
        .unwrap();
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let prompt_height = |visual: &mut gpui::VisualTestContext| {
        window
            .read_with(visual, |view, _| {
                let row = view
                    .rows
                    .iter()
                    .position(|r| matches!(r.kind, crate::rows::RowKind::User(0)))
                    .expect("prompt row");
                view.list_state
                    .bounds_for_item(row)
                    .expect("prompt row measured")
                    .size
                    .height
            })
            .unwrap()
    };
    let click_toggle = |visual: &mut gpui::VisualTestContext| {
        let toggle = visual
            .debug_bounds("agent-chat-user-fold-0")
            .expect("the toggle sits under the prompt");
        // Right-aligned in a full-width row: press just inside its right end.
        let at = gpui::point(toggle.right() - gpui::px(8.), toggle.center().y);
        visual.simulate_click(at, gpui::Modifiers::default());
        visual.run_until_parked();
    };

    let folded = prompt_height(&mut visual);
    click_toggle(&mut visual);
    let open = prompt_height(&mut visual);
    assert!(
        open > folded,
        "opening shows the whole prompt: {folded:?} -> {open:?}"
    );
    click_toggle(&mut visual);
    assert_eq!(
        prompt_height(&mut visual),
        folded,
        "folding it back restores the preview"
    );
}

/// Lay out one settled prompt and return the bounds of its bubble and of each
/// named control, so a test can say which element a control belongs to.
fn bubble_and_controls(
    prompt: String,
    controls: &[&'static str],
    cx: &mut gpui::TestAppContext,
) -> (
    gpui::Bounds<gpui::Pixels>,
    Vec<Option<gpui::Bounds<gpui::Pixels>>>,
) {
    let window = make_test_view(cx);
    window
        .update(cx, |view, _window, cx| {
            view.items = vec![ChatItem::UserText(prompt), assistant_text_item("ok")];
            view.settle_prompts();
            view.rebuild_rows();
            cx.notify();
        })
        .unwrap();
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let bubble = visual
        .debug_bounds("agent-chat-user-bubble-0")
        .expect("the prompt bubble is laid out");
    let found = controls.iter().map(|c| visual.debug_bounds(c)).collect();
    (bubble, found)
}

fn inside(outer: gpui::Bounds<gpui::Pixels>, inner: gpui::Bounds<gpui::Pixels>) -> bool {
    outer.left() <= inner.left()
        && inner.right() <= outer.right()
        && outer.top() <= inner.top()
        && inner.bottom() <= outer.bottom()
}

/// The fold toggle and copy button belong to the prompt, so they sit inside
/// its bubble: outside it, they read as loose controls of the pane.
#[gpui::test]
fn a_folded_prompt_keeps_its_toggle_and_copy_inside_the_bubble(cx: &mut gpui::TestAppContext) {
    let (bubble, found) = bubble_and_controls(
        "pasted log line\n".repeat(40),
        &["agent-chat-user-fold-0", "agent-chat-user-copy-0"],
        cx,
    );
    let [Some(toggle), Some(copy)] = found[..] else {
        panic!("both controls are laid out: {found:?}");
    };
    assert!(inside(bubble, toggle), "toggle {toggle:?} in {bubble:?}");
    assert!(inside(bubble, copy), "copy {copy:?} in {bubble:?}");
    assert!(
        toggle.top() >= bubble.center().y && copy.top() >= bubble.center().y,
        "the controls form the bubble's footer"
    );
    assert!(
        toggle.right() <= copy.left(),
        "the toggle leads and copy trails"
    );
}

#[gpui::test]
fn a_short_prompt_keeps_its_copy_inside_the_bubble(cx: &mut gpui::TestAppContext) {
    let (bubble, found) = bubble_and_controls(
        "a short question".into(),
        &["agent-chat-user-copy-0", "agent-chat-user-fold-0"],
        cx,
    );
    let Some(copy) = found[0] else {
        panic!("the copy button is laid out");
    };
    assert!(inside(bubble, copy), "copy {copy:?} in {bubble:?}");
    assert!(found[1].is_none(), "a short prompt has no fold toggle");
    assert!(
        bubble.size.width < gpui::px(300.),
        "a short prompt's bubble hugs its text: {bubble:?}"
    );
}

/// A short prompt still wraps inside its bubble now that copy trails it in a
/// row: a row child that refuses to shrink would run the text past the edge.
#[gpui::test]
fn a_wrapping_short_prompt_stays_inside_its_bubble(cx: &mut gpui::TestAppContext) {
    let prompt = "words that wrap across the bubble ".repeat(12);
    assert!(prompt.chars().count() < crate::PROMPT_FOLD_CHARS);
    let (bubble, found) = bubble_and_controls(prompt, &["agent-chat-user-copy-0"], cx);
    let Some(copy) = found[0] else {
        panic!("the copy button is laid out");
    };
    // Text that will not shrink keeps the bubble at its 85% cap and pushes the
    // trailing copy past the bubble's edge.
    assert!(inside(bubble, copy), "copy {copy:?} in {bubble:?}");
}
