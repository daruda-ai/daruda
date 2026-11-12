//! Resource preview ownership, admission, and decoder generation bookkeeping.

use super::resource_image_ops::{ResourceImageDimensions, ResourceImageViewport};
use crate::reconcile::{ReconcileScope, ResourceImageError};
use daruda_ui::CachedImage;
use std::cell::Cell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;

const MAX_INFLIGHT: usize = 2;
const MAX_BYTES: usize = 256 * 1024 * 1024;
const RESERVATION: usize = daruda_content::local_image::MAX_ALLOC_BYTES as usize;
mod lease;
use lease::{ImageOwner, ImageState, Maintenance};
pub(crate) use lease::{ResourceImageLease, ResourceModalLease};

/// Identifies one actual worker, independently of source identity and reset.
pub(crate) struct ResourceImageJob {
    id: u64,
    generation: u64,
    key: String,
    source: u64,
}

#[derive(Default)]
pub(crate) struct ResourceImageCache {
    images: HashMap<String, Result<ResourceImageLease, ResourceImageError>>,
    order: VecDeque<String>,
    deferred: HashSet<String>,
    visible: HashSet<String>,
    viewport: ResourceImageViewport,
    dimensions: HashMap<String, ResourceImageDimensions>,
    discarded: Vec<ResourceImageLease>,
    sources: HashMap<String, u64>,
    jobs: HashMap<u64, (String, u64)>,
    generation: u64,
    next_job: u64,
    maintenance: Maintenance,
}
impl ResourceImageCache {
    #[cfg(test)]
    pub(crate) fn entries(
        &self,
    ) -> &HashMap<String, Result<ResourceImageLease, ResourceImageError>> {
        &self.images
    }
    pub(crate) fn lookup(
        &self,
        key: &str,
    ) -> Option<Result<ResourceImageLease, ResourceImageError>> {
        self.images.get(key).cloned()
    }
    pub(crate) fn viewport(&self) -> &ResourceImageViewport {
        &self.viewport
    }
    pub(crate) fn dimensions(&self, key: &str) -> Option<&ResourceImageDimensions> {
        self.dimensions.get(key)
    }
    #[cfg(test)]
    pub(crate) fn source_count(&self) -> usize {
        self.sources.len()
    }
    pub(crate) fn inflight_count(&self) -> usize {
        self.jobs.len()
    }
    #[cfg(test)]
    pub(crate) fn geometry_count(&self) -> usize {
        self.dimensions.len()
    }
    #[cfg(test)]
    pub(crate) fn oldest(&self) -> Option<&String> {
        self.order.front()
    }
    #[cfg(test)]
    pub(crate) fn is_deferred(&self, key: &str) -> bool {
        self.deferred.contains(key)
    }
    pub(crate) fn needs_release(&self) -> bool {
        self.discarded
            .iter()
            .any(|image| Rc::strong_count(&image.0) == 1)
    }
    pub(crate) fn install_maintenance(&mut self, callback: impl Fn(Option<CachedImage>) + 'static) {
        *self.maintenance.borrow_mut() = Some(Rc::new(callback));
    }
    pub(crate) fn set_visible(&mut self, visible: HashSet<String>) -> bool {
        if self.visible == visible {
            return false;
        }
        for key in visible.difference(&self.visible) {
            self.deferred.remove(key);
        }
        self.visible = visible;
        true
    }
    pub(crate) fn sync_viewport(&mut self) -> bool {
        self.set_visible(self.viewport.keys())
    }
    pub(crate) fn clear(&mut self) {
        self.generation = self
            .generation
            .checked_add(1)
            .expect("resource generation exhausted");
        for key in self.images.keys().cloned().collect::<Vec<_>>() {
            self.discard(&key);
        }
        self.sources.clear();
        self.visible.clear();
        self.order.clear();
        self.deferred.clear();
        self.dimensions.clear();
        self.viewport.begin_frame(gpui::Bounds::default());
        // Detached workers still own memory. Their reservations survive reset.
    }
    pub(crate) fn reconcile_sources(&mut self, live: &HashSet<String>, scope: &ReconcileScope) {
        let stale = crate::reconcile::stale_keys(self.sources.keys(), live, scope);
        for key in stale {
            self.sources.remove(&key);
            self.dimensions.remove(&key);
            self.deferred.remove(&key);
            self.discard(&key);
        }
    }
    pub(crate) fn wants(&mut self, key: &str, source: u64) -> bool {
        if self.sources.get(key) != Some(&source) {
            self.sources.insert(key.to_owned(), source);
            self.dimensions.remove(key);
            self.deferred.remove(key);
            self.discard(key);
        }
        if !self.visible.contains(key) {
            self.discard(key);
            return false;
        }
        !self.deferred.contains(key)
            && !self.images.contains_key(key)
            && !self.jobs.values().any(|(running, _)| running == key)
    }
    pub(crate) fn slots(&self) -> usize {
        slots(self.resident_bytes(), self.jobs.len())
    }
    pub(crate) fn reserve(&mut self, key: String, source: u64) -> Option<ResourceImageJob> {
        if self.slots() == 0 || !self.wants(&key, source) {
            return None;
        }
        self.next_job = self
            .next_job
            .checked_add(1)
            .expect("resource job identity exhausted");
        self.jobs.insert(self.next_job, (key.clone(), source));
        Some(ResourceImageJob {
            id: self.next_job,
            generation: self.generation,
            key,
            source,
        })
    }
    pub(crate) fn complete(
        &mut self,
        job: ResourceImageJob,
        image: Result<CachedImage, ResourceImageError>,
    ) -> bool {
        if self.jobs.remove(&job.id).is_none() {
            return false;
        }
        if job.generation != self.generation
            || self.sources.get(&job.key) != Some(&job.source)
            || !self.visible.contains(&job.key)
        {
            return false;
        }
        self.cache(job.key, image);
        true
    }
    pub(crate) fn cache(&mut self, key: String, image: Result<CachedImage, ResourceImageError>) {
        self.discard(&key);
        let image = image.map(|image| {
            self.order.push_back(key.clone());
            let size = image.render_image().size(0);
            self.dimensions.insert(
                key.clone(),
                ResourceImageDimensions {
                    width: image.block_width(),
                    aspect_ratio: size.width.0 as f32 / (size.height.0 as f32).max(1.),
                },
            );
            ResourceImageLease(Rc::new(ImageOwner {
                image,
                state: Cell::new(ImageState::Resident),
                modal_pins: Cell::new(0),
                maintenance: self.maintenance.clone(),
            }))
        });
        self.images.insert(key, image);
    }
    pub(crate) fn discard(&mut self, key: &str) {
        self.order.retain(|resident| resident != key);
        if let Some(Ok(image)) = self.images.remove(key) {
            image.0.state.set(ImageState::Retired);
            self.discarded.push(image);
        }
    }
    pub(crate) fn evict_oldest(&mut self) -> Option<usize> {
        while let Some(key) = self.order.pop_front() {
            if let Some(Ok(image)) = self.images.get(&key) {
                let bytes = bytes(image);
                self.discard(&key);
                self.deferred.insert(key);
                return Some(bytes);
            }
        }
        None
    }
    /// Retire FIFO residents until releasing unpinned discards opens a slot.
    /// Returns whether that slot will open once those releases land.
    pub(crate) fn prepare_admission(&mut self) -> bool {
        let mut after_release = self
            .resident_bytes()
            .saturating_sub(self.projected_release_bytes());
        while slots(after_release, self.jobs.len()) == 0 {
            let Some(freed) = self.evict_oldest() else {
                break;
            };
            // Frame pins expire on repaint; modal pins can survive indefinitely.
            if self
                .discarded
                .last()
                .is_some_and(|image| image.0.modal_pins.get() == 0)
            {
                after_release = after_release.saturating_sub(freed);
            }
        }
        slots(after_release, self.jobs.len()) > 0
    }
    pub(crate) fn retry_budget_failures(&mut self) {
        let retry: Vec<_> = self
            .images
            .iter()
            .filter(|(key, image)| {
                self.visible.contains(*key) && matches!(image, Err(ResourceImageError::Budget))
            })
            .map(|(key, _)| key.clone())
            .collect();
        for key in retry {
            self.discard(&key);
        }
    }
    pub(crate) fn resident_bytes(&self) -> usize {
        self.images
            .values()
            .filter_map(|entry| entry.as_ref().ok())
            .chain(self.discarded.iter())
            .map(bytes)
            .sum()
    }
    fn projected_release_bytes(&self) -> usize {
        self.discarded
            .iter()
            .filter(|image| image.0.modal_pins.get() == 0)
            .map(bytes)
            .sum()
    }
    pub(crate) fn take_releasable(&mut self) -> Vec<CachedImage> {
        let mut released = Vec::new();
        self.discarded.retain(|image| {
            if Rc::strong_count(&image.0) > 1 {
                return true;
            }
            image.0.state.set(ImageState::Released);
            released.push(image.0.image.clone());
            false
        });
        released
    }
}
fn bytes(image: &ResourceImageLease) -> usize {
    image.render_image().as_bytes(0).map_or(0, <[u8]>::len)
}
fn slots(resident: usize, inflight: usize) -> usize {
    MAX_INFLIGHT.saturating_sub(inflight).min(
        MAX_BYTES.saturating_sub(resident.saturating_add(inflight.saturating_mul(RESERVATION)))
            / RESERVATION,
    )
}

#[cfg(test)]
mod tests;
