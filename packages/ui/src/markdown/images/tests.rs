use super::*;

#[test]
fn local_paths_resolve_without_granting_data_or_network_access() {
    let base = std::env::current_dir().unwrap();
    let absolute = base.join("preview.png");
    let file_url = daruda_core::file_url::from_local_path(&absolute).unwrap();
    for uri in [
        "preview.png".to_string(),
        absolute.to_string_lossy().into_owned(),
        file_url,
    ] {
        assert_eq!(
            local_path(&SharedUri::from(uri), Some(&base)),
            Some(absolute.clone())
        );
    }
    let uri = SharedUri::from(absolute.to_string_lossy().into_owned());
    assert_eq!(local_path(&uri, None), Some(absolute));
    for uri in [
        "preview.png",
        "https://example.com/image.png",
        "data:image/png;base64,aW1hZ2U=",
    ] {
        assert!(local_path(&SharedUri::from(uri), None).is_none());
    }
}

struct Probe;
impl gpui::Render for Probe {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
        gpui::div()
    }
}

#[gpui::test]
fn clearing_host_state_releases_pixels_even_when_a_source_binding_survives(
    cx: &mut gpui::TestAppContext,
) {
    crate::test_support::init_gpui_component(cx);
    let window = cx.add_window(|_, _| Probe);
    let states = crate::MarkdownStates::default();
    let (weak, binding) = window
        .update(cx, |_, window, cx| {
            let store = states.local_image_store(window, cx);
            let weak = store.cache.downgrade();
            let lease = cx.new(|cx| ImageLease::new(&store, cx));
            let binding = store.sources(None, &lease);
            (weak, binding)
        })
        .unwrap();
    assert!(weak.upgrade().is_some());
    states.clear();
    cx.run_until_parked();
    assert!(
        weak.upgrade().is_none(),
        "render sources must not retain the host cache"
    );
    assert!(binding.source(&SharedUri::from("/gone.png")).is_some());
}

#[gpui::test]
fn viewport_lease_does_not_keep_the_host_cache_alive(cx: &mut gpui::TestAppContext) {
    let window = cx.add_window(|_, _| Probe);
    let (weak, lease) = window
        .update(cx, |_, window, cx| {
            let store = ImageStore::new(window, cx);
            let weak = store.cache.downgrade();
            let lease = cx.new(|cx| ImageLease::new(&store, cx));
            (weak, lease)
        })
        .unwrap();
    cx.update(|_| {});
    assert!(weak.upgrade().is_none());
    drop(lease);
    cx.update(|_| {});
}

struct ResetProbe {
    states: crate::MarkdownStates,
    text: gpui::SharedString,
    drawn: bool,
    base: Option<PathBuf>,
}

impl gpui::Render for ResetProbe {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
        use gpui::ParentElement as _;
        gpui::div().children(self.drawn.then(|| {
            crate::markdown(
                "body",
                self.text.clone(),
                crate::CopyTooltips {
                    copy: "Copy".into(),
                    copied: "Copied".into(),
                },
            )
            .retained_in(&self.states)
            .local_images(self.base.as_deref())
        }))
    }
}

#[gpui::test]
async fn host_reset_replaces_the_visible_body_lease(cx: &mut gpui::TestAppContext) {
    crate::test_support::init_gpui_component(cx);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("preview.png");
    image::RgbaImage::new(1, 1).save(&path).unwrap();
    let states = crate::MarkdownStates::default();
    let probe_states = states.clone();
    let text = format!("![preview]({})", path.display()).into();
    let window = cx.add_window(move |_, _| ResetProbe {
        states: probe_states,
        text,
        drawn: true,
        base: None,
    });
    cx.run_until_parked();
    let first = window
        .update(cx, |_, window, cx| states.local_image_store(window, cx))
        .unwrap();
    let old = first.cache.read_with(cx, |cache, _| cache.lease_ids(&path));
    assert_eq!(old.len(), 1);
    states.clear();
    let probe = window.root(cx).unwrap();
    probe.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    let second = window
        .update(cx, |_, window, cx| states.local_image_store(window, cx))
        .unwrap();
    let current = second
        .cache
        .read_with(cx, |cache, _| cache.lease_ids(&path));
    assert_eq!(current.len(), 1);
    assert_ne!(
        old, current,
        "reset must replace the release registration even when the body stays visible"
    );
    probe.update(cx, |probe, cx| {
        probe.drawn = false;
        cx.notify();
    });
    cx.run_until_parked();
    cx.update(|_| {});
    cx.run_until_parked();
    second
        .cache
        .read_with(cx, |cache, _| assert!(cache.lease_ids(&path).is_empty()));
}

#[gpui::test]
async fn replacing_body_content_releases_old_image_registrations(cx: &mut gpui::TestAppContext) {
    crate::test_support::init_gpui_component(cx);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("preview.png");
    image::RgbaImage::new(1, 1).save(&path).unwrap();
    let states = crate::MarkdownStates::default();
    let probe_states = states.clone();
    let text = format!("![preview]({})", path.display()).into();
    let window = cx.add_window(move |_, _| ResetProbe {
        states: probe_states,
        text,
        drawn: true,
        base: None,
    });
    cx.run_until_parked();
    let store = window
        .update(cx, |_, window, cx| states.local_image_store(window, cx))
        .unwrap();
    assert_eq!(
        store
            .cache
            .read_with(cx, |cache, _| cache.lease_ids(&path).len()),
        1
    );
    window.root(cx).unwrap().update(cx, |probe, cx| {
        probe.text = "No image anymore".into();
        cx.notify();
    });
    cx.run_until_parked();
    cx.update(|_| {});
    store
        .cache
        .read_with(cx, |cache, _| assert!(cache.lease_ids(&path).is_empty()));
}

#[gpui::test]
async fn changing_body_base_releases_old_relative_image_registrations(
    cx: &mut gpui::TestAppContext,
) {
    crate::test_support::init_gpui_component(cx);
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let old = first.path().join("preview.png");
    image::RgbaImage::new(1, 1).save(&old).unwrap();
    image::RgbaImage::new(1, 1)
        .save(second.path().join("preview.png"))
        .unwrap();
    let states = crate::MarkdownStates::default();
    let probe_states = states.clone();
    let base = first.path().to_path_buf();
    let window = cx.add_window(move |_, _| ResetProbe {
        states: probe_states,
        text: "![preview](preview.png)".into(),
        drawn: true,
        base: Some(base),
    });
    cx.run_until_parked();
    let store = window
        .update(cx, |_, window, cx| states.local_image_store(window, cx))
        .unwrap();
    assert_eq!(
        store
            .cache
            .read_with(cx, |cache, _| cache.lease_ids(&old).len()),
        1
    );
    window.root(cx).unwrap().update(cx, |probe, cx| {
        probe.base = Some(second.path().to_path_buf());
        cx.notify();
    });
    cx.run_until_parked();
    cx.update(|_| {});
    store
        .cache
        .read_with(cx, |cache, _| assert!(cache.lease_ids(&old).is_empty()));
}

#[gpui::test]
async fn streaming_text_keeps_a_retired_image_deferred(cx: &mut gpui::TestAppContext) {
    crate::test_support::init_gpui_component(cx);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("preview.png");
    image::RgbaImage::new(1, 1).save(&path).unwrap();
    let states = crate::MarkdownStates::default();
    let probe_states = states.clone();
    let text = format!("![preview]({})", path.display());
    let first = text.clone().into();
    let window = cx.add_window(move |_, _| ResetProbe {
        states: probe_states,
        text: first,
        drawn: true,
        base: None,
    });
    cx.run_until_parked();
    let store = window
        .update(cx, |_, window, cx| states.local_image_store(window, cx))
        .unwrap();
    store
        .cache
        .update(cx, |cache, _| cache.retire_for_test(&path));
    window.root(cx).unwrap().update(cx, |probe, cx| {
        probe.text = format!("{text}\n\nMore streamed text.").into();
        cx.notify();
    });
    cx.run_until_parked();
    store.cache.read_with(cx, |cache, _| {
        assert!(
            cache.is_deferred(&path),
            "a continuously visible body must not re-decode a retired image per delta"
        )
    });
}
