//! Multiple linked blocks must not overwrite one another's pointer input.
use super::*;
use std::sync::Mutex;

struct LinksProbe {
    opened: Arc<Mutex<Vec<String>>>,
    context: Arc<Mutex<Option<SharedString>>>,
    same_url: bool,
    occluded: bool,
    replaced: bool,
    block_capture: bool,
    consume_context: bool,
}

impl Render for LinksProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let context = self.context.clone();
        let block_capture = self.block_capture;
        let consume_context = self.consume_context;
        div()
            .capture_any_mouse_down(move |_, _, cx| {
                if block_capture {
                    cx.stop_propagation();
                }
            })
            .capture_any_mouse_up(move |_, _, cx| {
                if block_capture {
                    cx.stop_propagation();
                }
            })
            .size_full()
            .flex()
            .flex_col()
            .children((0usize..2).map(|ix| {
                let opened = self.opened.clone();
                div().h(px(40.)).child(
                    super::super::markdown(
                        ("multiple-links", ix),
                        SharedString::from(format!(
                            "[link](https://example.invalid/{})",
                            if self.same_url {
                                0
                            } else {
                                ix + usize::from(self.replaced) * 2
                            }
                        )),
                        copy_tooltips(),
                    )
                    .link_click_handler(move |url, _, _| {
                        opened.lock().unwrap().push(url.to_owned());
                        true
                    }),
                )
            }))
            .when(self.occluded, |this| {
                this.child(deferred(
                    div().absolute().inset_0().occlude().child("panel"),
                ))
            })
            .on_mouse_down(MouseButton::Right, move |event, window, cx| {
                if consume_context {
                    *context.lock().unwrap() =
                        crate::take_right_clicked_link(cx, window, event.position);
                }
            })
    }
}

type LinkLog = Arc<Mutex<Vec<String>>>;
type ContextLog = Arc<Mutex<Option<SharedString>>>;

fn new_links(cx: &mut TestAppContext) -> (VisualTestContext, LinkLog, ContextLog) {
    init_gpui_component(cx);
    let opened = Arc::new(Mutex::new(Vec::new()));
    let context = Arc::new(Mutex::new(None));
    let window = cx.update({
        let opened = opened.clone();
        let context = context.clone();
        move |cx| {
            cx.open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| LinksProbe {
                    opened,
                    context,
                    same_url: false,
                    occluded: false,
                    replaced: false,
                    block_capture: false,
                    consume_context: true,
                })
            })
            .unwrap()
        }
    });
    let vcx = VisualTestContext::from_window(window.into(), cx);
    vcx.run_until_parked();
    (vcx, opened, context)
}

fn with_links(
    cx: &mut TestAppContext,
    drive: impl FnOnce(&mut VisualTestContext, LinkLog, ContextLog),
) {
    let (mut vcx, opened, context) = new_links(cx);
    drive(&mut vcx, opened, context);
}

#[gpui::test]
fn multiple_link_blocks_preserve_each_activation(cx: &mut TestAppContext) {
    with_links(cx, |vcx, opened, _| {
        for ix in 0..2 {
            let at = point(px(8.), px(8. + ix as f32 * 40.));
            press(vcx, at, MouseButton::Left);
            release(vcx, at, MouseButton::Left);
        }
        assert_eq!(
            *opened.lock().unwrap(),
            ["https://example.invalid/0", "https://example.invalid/1"]
        );
    });
}

#[gpui::test]
fn multiple_link_blocks_preserve_each_context_address(cx: &mut TestAppContext) {
    with_links(cx, |vcx, _, context| {
        for ix in 0..2 {
            press(
                vcx,
                point(px(8.), px(8. + ix as f32 * 40.)),
                MouseButton::Right,
            );
            assert_eq!(
                context.lock().unwrap().as_deref(),
                Some(format!("https://example.invalid/{ix}").as_str())
            );
        }
    });
}

fn change_probe(vcx: &mut VisualTestContext, change: impl FnOnce(&mut LinksProbe)) {
    vcx.update(|window, cx| {
        window
            .root::<LinksProbe>()
            .flatten()
            .unwrap()
            .update(cx, |probe, cx| {
                change(probe);
                cx.notify();
            });
    });
    vcx.run_until_parked();
}

#[gpui::test]
fn same_url_links_do_not_adopt_a_drag(cx: &mut TestAppContext) {
    with_links(cx, |vcx, opened, _| {
        change_probe(vcx, |probe| probe.same_url = true);
        press(vcx, on_link(), MouseButton::Left);
        release(vcx, point(px(8.), px(48.)), MouseButton::Left);
        assert!(opened.lock().unwrap().is_empty());
    });
}

#[gpui::test]
fn outside_release_clears_link_record(cx: &mut TestAppContext) {
    with_links(cx, |vcx, opened, _| {
        press(vcx, on_link(), MouseButton::Left);
        release(vcx, point(px(200.), px(200.)), MouseButton::Left);
        release(vcx, on_link(), MouseButton::Left);
        assert!(opened.lock().unwrap().is_empty());
    });
}

#[gpui::test]
fn outside_press_clears_link_record(cx: &mut TestAppContext) {
    with_links(cx, |vcx, opened, _| {
        press(vcx, on_link(), MouseButton::Left);
        press(vcx, point(px(200.), px(200.)), MouseButton::Left);
        release(vcx, on_link(), MouseButton::Left);
        assert!(opened.lock().unwrap().is_empty());
    });
}

#[gpui::test]
fn occluded_press_clears_link_record(cx: &mut TestAppContext) {
    with_links(cx, |vcx, opened, _| {
        press(vcx, on_link(), MouseButton::Left);
        change_probe(vcx, |probe| probe.occluded = true);
        press(vcx, on_link(), MouseButton::Left);
        change_probe(vcx, |probe| probe.occluded = false);
        release(vcx, on_link(), MouseButton::Left);
        assert!(opened.lock().unwrap().is_empty());
    });
}

#[gpui::test]
fn redraw_preserves_unchanged_link_press(cx: &mut TestAppContext) {
    with_links(cx, |vcx, opened, _| {
        press(vcx, on_link(), MouseButton::Left);
        change_probe(vcx, |_| {});
        release(vcx, on_link(), MouseButton::Left);
        assert_eq!(*opened.lock().unwrap(), ["https://example.invalid/0"]);
    });
}

#[gpui::test]
fn stream_replacement_rejects_old_press(cx: &mut TestAppContext) {
    with_links(cx, |vcx, opened, _| {
        press(vcx, on_link(), MouseButton::Left);
        change_probe(vcx, |probe| probe.replaced = true);
        release(vcx, on_link(), MouseButton::Left);
        assert!(opened.lock().unwrap().is_empty());
    });
}

#[gpui::test]
fn context_address_expires_after_its_mouse_down_dispatch(cx: &mut TestAppContext) {
    with_links(cx, |vcx, _, context| {
        press(vcx, on_link(), MouseButton::Right);
        assert_eq!(
            context.lock().unwrap().take().as_deref(),
            Some("https://example.invalid/0")
        );

        change_probe(vcx, |probe| probe.consume_context = false);
        press(vcx, on_link(), MouseButton::Right);
        assert!(context.lock().unwrap().is_none());
        let stale = vcx.update(|window, cx| crate::take_right_clicked_link(cx, window, on_link()));
        assert_eq!(stale, None);
    });
}

#[gpui::test]
fn windows_do_not_share_link_records(cx: &mut TestAppContext) {
    let (mut first, first_opened, _) = new_links(cx);
    let (mut second, second_opened, _) = new_links(cx);
    press(&mut first, on_link(), MouseButton::Left);
    press(&mut second, on_link(), MouseButton::Left);
    release(&mut first, on_link(), MouseButton::Left);
    release(&mut second, on_link(), MouseButton::Left);
    assert_eq!(*first_opened.lock().unwrap(), ["https://example.invalid/0"]);
    assert_eq!(
        *second_opened.lock().unwrap(),
        ["https://example.invalid/0"]
    );
}

#[gpui::test]
fn root_backed_hosts_preserve_both_link_gestures(cx: &mut TestAppContext) {
    init_gpui_component(cx);
    let opened = Arc::new(Mutex::new(Vec::new()));
    let context = Arc::new(Mutex::new(None));
    let (_, vcx) = cx.add_window_view({
        let opened = opened.clone();
        let context = context.clone();
        move |window, cx| {
            let probe = cx.new(|_| LinksProbe {
                opened,
                context,
                same_url: false,
                occluded: false,
                replaced: false,
                block_capture: false,
                consume_context: true,
            });
            crate::Root::new(probe, window, cx)
        }
    });
    vcx.run_until_parked();
    for ix in 0..2 {
        let at = point(px(8.), px(8. + ix as f32 * 40.));
        press(vcx, at, MouseButton::Left);
        release(vcx, at, MouseButton::Left);
        press(vcx, at, MouseButton::Right);
        assert_eq!(
            context.lock().unwrap().as_deref(),
            Some(format!("https://example.invalid/{ix}").as_str())
        );
    }
    assert_eq!(
        *opened.lock().unwrap(),
        ["https://example.invalid/0", "https://example.invalid/1"]
    );
}

#[gpui::test]
fn stopped_capture_cannot_leave_a_press_for_a_later_release(cx: &mut TestAppContext) {
    for block_next_down in [false, true] {
        with_links(cx, |vcx, opened, _| {
            press(vcx, on_link(), MouseButton::Left);
            change_probe(vcx, |probe| probe.block_capture = true);
            release(vcx, point(px(200.), px(200.)), MouseButton::Left);
            if block_next_down {
                press(vcx, point(px(200.), px(200.)), MouseButton::Left);
            }
            change_probe(vcx, |probe| probe.block_capture = false);
            release(vcx, on_link(), MouseButton::Left);
            assert!(
                opened.lock().unwrap().is_empty(),
                "blocked input must not reuse a prior press"
            );
        });
    }
}
