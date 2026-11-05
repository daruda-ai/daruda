//! Independent RenderContext hosts need no Base Root for link gestures.
use super::*;
use gpui::{
    InteractiveElement as _, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, SharedString,
    Styled as _, point, px,
};
use std::sync::Mutex;

struct LinksHost {
    blocks: Vec<BlockNode>,
    context: RenderContext,
    right: Arc<Mutex<Option<SharedString>>>,
}

impl Render for LinksHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let blocks: Vec<_> = self.blocks.iter().collect();
        let right = self.right.clone();
        div()
            .size_full()
            .children(self.blocks.iter().enumerate().map(|(ix, block)| {
                div().h(px(40.)).child(block.render_with_id(
                    ("host-link", ix),
                    FlowPosition::at(&blocks, ix, ix + 1 == blocks.len()),
                    &self.context,
                    window,
                    cx,
                ))
            }))
            .on_mouse_down(MouseButton::Right, move |event, window, cx| {
                *right.lock().unwrap() =
                    gpui_base::take_right_clicked_link(cx, window, event.position);
            })
    }
}

#[gpui::test]
fn independent_host_opens_and_copies_both_links(cx: &mut TestAppContext) {
    cx.update(gpui_base::init);
    let opened = Arc::new(Mutex::new(Vec::new()));
    let right = Arc::new(Mutex::new(None));
    let context = RenderContext::new(TextViewStyle::default()).on_link_click({
        let opened = opened.clone();
        move |url, _, _, _| opened.lock().unwrap().push(url.to_string())
    });
    let blocks = parse_markdown(
        "[first](https://example.invalid/0)\n\n[second](https://example.invalid/1)",
        Default::default(),
    )
    .unwrap();
    let (_, mut vcx) = cx.add_window_view({
        let right = right.clone();
        move |_, _| LinksHost {
            blocks,
            context,
            right,
        }
    });
    vcx.run_until_parked();
    for ix in 0..2 {
        let at = point(px(8.), px(8. + ix as f32 * 40.));
        vcx.simulate_event(MouseDownEvent {
            position: at,
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        });
        vcx.simulate_event(MouseUpEvent {
            position: at,
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
            click_count: 1,
        });
        vcx.simulate_event(MouseDownEvent {
            position: at,
            button: MouseButton::Right,
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        });
        assert_eq!(
            right.lock().unwrap().as_deref(),
            Some(format!("https://example.invalid/{ix}").as_str())
        );
    }
    assert_eq!(
        *opened.lock().unwrap(),
        ["https://example.invalid/0", "https://example.invalid/1"]
    );
}
