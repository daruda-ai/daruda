//! Host-owned local rasters with bounded decoding and explicit atlas retirement.

mod jobs;
mod policy;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{AnyWindowHandle, Context, EntityId, ImageCacheError, RenderImage, Task, Window};

use super::version::Version;

const CACHE_BYTES: usize = 256 * 1024 * 1024;
const JOB_BYTES: usize = daruda_content::local_image::MAX_ALLOC_BYTES as usize;
const MAX_JOBS: usize = 2;

type ImageResult = Result<Arc<RenderImage>, ImageCacheError>;

enum State {
    Queued,
    Loading,
    Ready(ImageResult),
    Deferred,
}

struct Entry {
    generation: u64,
    version: Option<Version>,
    state: State,
    leases: HashMap<EntityId, HashSet<EntityId>>,
    workers: usize,
    blocked: HashSet<EntityId>,
}

pub(super) struct LocalImageCache {
    window: AnyWindowHandle,
    entries: HashMap<PathBuf, Entry>,
    loaded: VecDeque<PathBuf>,
    bytes: usize,
    running: usize,
    discarded_bytes: usize,
    discarded: Vec<Arc<RenderImage>>,
    maintenance_scheduled: bool,
    poller: Option<Task<()>>,
}

impl Entry {
    fn should_poll(&self) -> bool {
        matches!(self.state, State::Ready(Ok(_)))
            || (!self.leases.is_empty() && !matches!(self.state, State::Deferred))
    }
}

impl LocalImageCache {
    pub(super) fn new(window: AnyWindowHandle, cx: &mut Context<Self>) -> Self {
        cx.on_release(move |cache, cx| {
            let mut images = std::mem::take(&mut cache.discarded);
            for entry in cache.entries.values_mut() {
                if let State::Ready(Ok(image)) = &entry.state {
                    images.push(image.clone());
                }
            }
            // Release runs inside entity teardown; window access waits until it completes.
            cx.defer(move |cx| {
                // SILENT-OK: A closed window has already released its entire sprite atlas.
                let _ = window.update(cx, |_, window, _| {
                    for image in images {
                        // SILENT-OK: Pinned GPUI drop_image only removes atlas entries and returns Ok.
                        let _ = window.drop_image(image);
                    }
                });
            });
        })
        .detach();
        Self {
            window,
            entries: HashMap::new(),
            loaded: VecDeque::new(),
            bytes: 0,
            running: 0,
            discarded_bytes: 0,
            discarded: Vec::new(),
            maintenance_scheduled: false,
            poller: None,
        }
    }

    pub(super) fn request(
        &mut self,
        path: PathBuf,
        lease: EntityId,
        consumer: EntityId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<ImageResult> {
        let entry = self.entries.entry(path).or_insert_with(|| Entry {
            generation: 0,
            version: None,
            state: State::Queued,
            leases: HashMap::new(),
            workers: 0,
            blocked: HashSet::new(),
        });
        entry.leases.entry(lease).or_default().insert(consumer);
        if matches!(entry.state, State::Deferred) && !entry.blocked.contains(&lease) {
            entry.state = State::Queued;
        }
        let result = match &entry.state {
            State::Ready(result) => Some(result.clone()),
            _ => None,
        };
        self.start_poller(cx);
        self.pump(window, cx);
        result
    }

    pub(super) fn release(&mut self, lease: EntityId) {
        for entry in self.entries.values_mut() {
            entry.leases.remove(&lease);
            entry.blocked.remove(&lease);
        }
        self.prune_inactive();
    }

    /// A still-mounted body that changes content stays the same viewer, so its
    /// replacement lease keeps the retirements the old lease already saw.
    pub(super) fn inherit_blocks(&mut self, from: EntityId, to: EntityId) {
        for entry in self.entries.values_mut() {
            if entry.blocked.contains(&from) {
                entry.blocked.insert(to);
            }
        }
    }

    fn prune_inactive(&mut self) {
        self.entries.retain(|_, entry| {
            !entry.leases.is_empty()
                || entry.workers > 0
                || matches!(entry.state, State::Ready(Ok(_)))
        });
    }

    fn notify(entry: &Entry, cx: &mut Context<Self>) {
        let consumers: HashSet<_> = entry.leases.values().flatten().copied().collect();
        for consumer in consumers {
            gpui::App::notify(cx, consumer);
        }
    }
}
