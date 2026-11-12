use super::*;
use gpui::{point, px, size};

#[test]
fn viewport_collects_only_intersecting_cards_and_resets_each_frame() {
    let viewport = ResourceImageViewport::default();
    viewport.begin_frame(Bounds::new(
        point(px(0.), px(100.)),
        size(px(100.), px(200.)),
    ));
    viewport.record(
        "visible",
        Bounds::new(point(px(0.), px(150.)), size(px(100.), px(50.))),
    );
    viewport.record(
        "above",
        Bounds::new(point(px(0.), px(0.)), size(px(100.), px(50.))),
    );
    viewport.record(
        "below",
        Bounds::new(point(px(0.), px(350.)), size(px(100.), px(50.))),
    );
    assert_eq!(viewport.keys(), HashSet::from(["visible".to_owned()]));
    viewport.begin_frame(Bounds::default());
    assert!(viewport.keys().is_empty());
}

#[gpui::test]
fn memory_pressure_evicts_oldest_and_suppresses_immediate_reload(cx: &mut gpui::TestAppContext) {
    let window = crate::view::tests::make_test_view(cx);
    let view = window.root(cx).unwrap();
    view.update(cx, |v, _| {
        for key in ["old", "middle", "new"] {
            let image = daruda_ui::CachedImage::from_raster(daruda_content::visual::RasterImage {
                width: 2,
                height: 1,
                bgra: vec![0; 8],
                scale: 1.,
            })
            .unwrap();
            v.assets.resource_images.cache(key.into(), Ok(image));
        }
        assert_eq!(v.assets.resource_images.evict_oldest(), Some(8));
        let images = v.assets.resource_images.entries();
        assert!(!images.contains_key("old"));
        assert!(images.contains_key("middle"));
        assert!(images.contains_key("new"));
        assert!(v.assets.resource_images.is_deferred("old"));
    });
}

#[gpui::test]
fn reset_keeps_running_decoder_reservations(cx: &mut gpui::TestAppContext) {
    let window = crate::view::tests::make_test_view(cx);
    let view = window.root(cx).unwrap();
    view.update(cx, |v, _| {
        v.assets
            .resource_images
            .set_visible(HashSet::from(["old#0".into()]));
        let _running = v.assets.resource_images.reserve("old#0".into(), 7).unwrap();
        v.assets.clear();
        assert_eq!(v.assets.resource_images.inflight_count(), 1);
    });
}

#[gpui::test]
fn held_preview_survives_retirement_until_last_consumer(cx: &mut gpui::TestAppContext) {
    let window = crate::view::tests::make_test_view(cx);
    window
        .update(cx, |v, window, _| {
            let image = daruda_ui::CachedImage::from_raster(daruda_content::visual::RasterImage {
                width: 2,
                height: 1,
                bgra: vec![0; 8],
                scale: 1.,
            })
            .unwrap();
            v.assets.resource_images.cache("held".into(), Ok(image));
            let held = v
                .assets
                .resource_images
                .entries()
                .get("held")
                .unwrap()
                .as_ref()
                .unwrap()
                .clone();
            v.assets.resource_images.discard("held");
            v.release_resource_images(window);
            assert_eq!(v.assets.resource_images.resident_bytes(), 8);
            drop(held);
            v.release_resource_images(window);
            assert_eq!(v.assets.resource_images.resident_bytes(), 0);
        })
        .unwrap();
}

struct ModalHost;
impl gpui::Render for ModalHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        gpui::div()
    }
}

#[gpui::test]
fn lightbox_holds_retired_preview_until_dialog_dismissal(cx: &mut gpui::TestAppContext) {
    use daruda_ui::WindowExt as _;
    use gpui::AppContext as _;
    let chat_window = crate::view::tests::make_test_view(cx);
    let view = chat_window.root(cx).unwrap();
    let modal_window = cx.add_window(|window, cx| {
        let content = cx.new(|_| ModalHost);
        daruda_ui::Root::new(content, window, cx)
    });
    let image = chat_window
        .update(cx, |v, window, cx| {
            v.sync_resource_image_viewport(window, cx);
            let image = daruda_ui::CachedImage::from_raster(daruda_content::visual::RasterImage {
                width: 2,
                height: 1,
                bgra: vec![0; 8],
                scale: 1.,
            })
            .unwrap();
            v.assets.resource_images.cache("held".into(), Ok(image));
            v.assets.resource_images.lookup("held").unwrap().unwrap()
        })
        .unwrap();
    modal_window
        .update(cx, |_, window, cx| {
            window.defer(cx, move |window, cx| {
                crate::render::image_lightbox::open_resource(image, window, cx)
            })
        })
        .unwrap();
    cx.run_until_parked();
    chat_window
        .update(cx, |v, window, _| {
            v.assets.clear();
            assert!(!v.release_resource_images(window));
            assert_eq!(v.assets.resource_images.resident_bytes(), 8);
        })
        .unwrap();
    modal_window
        .update(cx, |_, window, cx| {
            window.defer(cx, |window, cx| window.close_dialog(cx))
        })
        .unwrap();
    cx.run_until_parked();
    chat_window
        .update(cx, |v, window, _| {
            v.release_resource_images(window);
        })
        .unwrap();
    view.read_with(cx, |v, _| {
        assert_eq!(v.assets.resource_images.resident_bytes(), 0)
    });
}
