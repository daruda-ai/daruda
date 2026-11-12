//! Host-owned bounded local images and weak render-source bindings.

mod cache;
mod version;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cache::LocalImageCache;
use gpui::{App, AppContext as _, Context, Entity, ImageSource, SharedUri, WeakEntity, Window};

pub(super) struct LocalImages {
    pub(super) base: Option<PathBuf>,
}

/// The host or keyed element owns this handle. Retained parses and image
/// sources keep only a weak binding so they cannot prolong the pixel cache.
#[derive(Clone)]
pub(super) struct ImageStore {
    cache: Entity<LocalImageCache>,
}

impl ImageStore {
    pub(super) fn new(window: &Window, cx: &mut App) -> Self {
        let handle = window.window_handle();
        Self {
            cache: cx.new(|cx| LocalImageCache::new(handle, cx)),
        }
    }

    pub(super) fn lease_id(&self, body: impl std::fmt::Display) -> gpui::ElementId {
        gpui::ElementId::Name(format!("{body}-local-image-lease-{}", self.cache.entity_id()).into())
    }

    pub(super) fn sources(
        &self,
        base: Option<PathBuf>,
        lease: &Entity<ImageLease>,
    ) -> ImageSources {
        ImageSources {
            cache: self.cache.downgrade(),
            base,
            lease: lease.downgrade(),
        }
    }
}

/// Keyed state gives a body returning to the viewport a fresh lease without
/// retaining its cache or its pixels in the parsed TextView document.
pub(super) struct ImageLease;

impl ImageLease {
    pub(super) fn new(store: &ImageStore, cx: &mut Context<Self>) -> Self {
        let cache = store.cache.downgrade();
        let lease = cx.entity_id();
        cx.on_release(move |_, cx| {
            // Entity release may run while the cache is borrowed by a render update.
            cx.defer(move |cx| {
                // SILENT-OK: Host teardown already discarded all lease registrations.
                let _ = cache.update(cx, |cache, _| cache.release(lease));
            });
        })
        .detach();
        Self
    }
}

/// An unchanged body keeps its admission token; replacing content releases
/// registrations even when its keyed element remains continuously mounted.
pub(super) struct ImageLeaseBinding {
    text: gpui::SharedString,
    base: Option<PathBuf>,
    lease: Entity<ImageLease>,
}

impl ImageLeaseBinding {
    pub(super) fn new(
        store: &ImageStore,
        text: gpui::SharedString,
        base: Option<PathBuf>,
        cx: &mut App,
    ) -> Self {
        Self {
            text,
            base,
            lease: cx.new(|cx| ImageLease::new(store, cx)),
        }
    }

    pub(super) fn lease(
        &mut self,
        store: &ImageStore,
        text: gpui::SharedString,
        base: Option<&Path>,
        cx: &mut App,
    ) -> Entity<ImageLease> {
        let text_changed = (self.text.as_ptr() != text.as_ptr() || self.text.len() != text.len())
            && self.text != text;
        if text_changed || self.base.as_deref() != base {
            self.text = text;
            self.base = base.map(Path::to_path_buf);
            let previous = self.lease.entity_id();
            self.lease = cx.new(|cx| ImageLease::new(store, cx));
            let next = self.lease.entity_id();
            store
                .cache
                .update(cx, |cache, _| cache.inherit_blocks(previous, next));
        }
        self.lease.clone()
    }
}

pub(super) struct ImageSources {
    cache: WeakEntity<LocalImageCache>,
    base: Option<PathBuf>,
    lease: WeakEntity<ImageLease>,
}

impl ImageSources {
    pub(super) fn source(&self, uri: &SharedUri) -> Option<ImageSource> {
        let path = local_path(uri, self.base.as_deref())?;
        let cache = self.cache.clone();
        let lease = self.lease.clone();
        Some(ImageSource::Custom(Arc::new(move |window, cx| {
            let lease = lease.upgrade()?;
            let cache = cache.upgrade()?;
            let consumer = window.current_view();
            cache.update(cx, |cache, cx| {
                cache.request(path.clone(), lease.entity_id(), consumer, window, cx)
            })
        })))
    }
}

fn local_path(uri: &SharedUri, base: Option<&Path>) -> Option<PathBuf> {
    if uri.starts_with("data:") {
        return None;
    }
    daruda_content::link_target::resource_path(uri, base)
}

#[cfg(test)]
mod tests;
