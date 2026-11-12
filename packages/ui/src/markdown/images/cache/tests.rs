use gpui::{AppContext as _, Entity, IntoElement, Render, TestAppContext, WindowHandle, div};

use super::*;

pub(super) struct Probe;
impl Render for Probe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

pub(super) struct Rig {
    pub(super) cache: Entity<LocalImageCache>,
    pub(super) window: WindowHandle<Probe>,
    pub(super) lease: Entity<super::super::ImageLease>,
}

impl Rig {
    pub(super) fn new(cx: &mut TestAppContext) -> Self {
        let window = cx.add_window(|_, _| Probe);
        let cache = cx.new(|cx| LocalImageCache::new(window.into(), cx));
        let store = super::super::ImageStore {
            cache: cache.clone(),
        };
        let lease = cx.new(|cx| super::super::ImageLease::new(&store, cx));
        Self {
            cache,
            window,
            lease,
        }
    }

    pub(super) fn request(
        &self,
        path: &std::path::Path,
        cx: &mut TestAppContext,
    ) -> Option<ImageResult> {
        self.window
            .update(cx, |_, window, cx| {
                self.cache.update(cx, |cache, cx| {
                    cache.request(
                        path.into(),
                        self.lease.entity_id(),
                        self.lease.entity_id(),
                        window,
                        cx,
                    )
                })
            })
            .unwrap()
    }

    pub(super) fn poll(&self, cx: &mut TestAppContext) {
        cx.background_executor
            .advance_clock(Duration::from_millis(500));
        cx.run_until_parked();
    }
}

use std::time::Duration;

pub(super) fn write_image(path: &std::path::Path, width: u32) {
    image::RgbaImage::from_pixel(width, 1, image::Rgba([1, 2, 3, 255]))
        .save(path)
        .unwrap();
}

#[gpui::test]
async fn repeated_requests_share_one_decoder_and_ready_raster(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("preview.png");
    write_image(&path, 1);
    let rig = Rig::new(cx);
    assert!(rig.request(&path, cx).is_none());
    assert!(rig.request(&path, cx).is_none());
    rig.cache.read_with(cx, |cache, _| {
        assert_eq!(cache.entries.len(), 1);
        assert_eq!(cache.running, 1);
    });
    cx.run_until_parked();
    let first = rig.request(&path, cx).unwrap().unwrap();
    let second = rig.request(&path, cx).unwrap().unwrap();
    assert!(Arc::ptr_eq(&first, &second));
}

#[gpui::test]
async fn released_viewport_leases_do_not_accumulate_registrations(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("preview.png");
    write_image(&path, 1);
    let rig = Rig::new(cx);
    for _ in 0..8 {
        let store = super::super::ImageStore {
            cache: rig.cache.clone(),
        };
        let lease = cx.new(|cx| super::super::ImageLease::new(&store, cx));
        rig.window
            .update(cx, |_, window, cx| {
                rig.cache.update(cx, |cache, cx| {
                    cache.request(
                        path.clone(),
                        lease.entity_id(),
                        lease.entity_id(),
                        window,
                        cx,
                    );
                });
            })
            .unwrap();
        cx.run_until_parked();
        drop(lease);
        cx.update(|_| {});
        cx.run_until_parked();
        rig.cache.read_with(cx, |cache, _| {
            let entry = &cache.entries[&path];
            assert!(entry.leases.is_empty(), "released leases must unregister");
            assert!(entry.blocked.is_empty());
        });
    }
}

#[gpui::test]
async fn release_discards_inactive_metadata_but_preserves_resident_pixels(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let ready = dir.path().join("ready.png");
    let missing = dir.path().join("missing.png");
    write_image(&ready, 1);
    let rig = Rig::new(cx);
    rig.request(&ready, cx);
    rig.request(&missing, cx);
    cx.run_until_parked();
    let image = rig.request(&ready, cx).unwrap().unwrap();
    rig.cache
        .update(cx, |cache, _| cache.release(rig.lease.entity_id()));
    rig.cache.read_with(cx, |cache, _| {
        assert!(!cache.entries.contains_key(&missing));
        let State::Ready(Ok(resident)) = &cache.entries[&ready].state else {
            panic!("resident must survive viewport release")
        };
        assert!(Arc::ptr_eq(resident, &image));
    });
    rig.cache.update(cx, |cache, _| {
        cache.retire(&ready);
        cache.prune_inactive();
        assert!(!cache.entries.contains_key(&ready));
    });
}

impl LocalImageCache {
    pub(in crate::markdown::images) fn lease_ids(
        &self,
        path: &std::path::Path,
    ) -> HashSet<EntityId> {
        self.entries
            .get(path)
            .map(|entry| entry.leases.keys().copied().collect())
            .unwrap_or_default()
    }
}

impl LocalImageCache {
    pub(in crate::markdown::images) fn retire_for_test(&mut self, path: &std::path::Path) {
        self.retire(&path.to_path_buf());
    }

    pub(in crate::markdown::images) fn is_deferred(&self, path: &std::path::Path) -> bool {
        self.entries
            .get(path)
            .is_some_and(|entry| matches!(entry.state, State::Deferred))
    }
}
