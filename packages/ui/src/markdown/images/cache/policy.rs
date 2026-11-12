//! Admission and retirement retain reservations until atlas cleanup completes.

#[cfg(test)]
mod tests;

use super::*;

impl LocalImageCache {
    pub(super) fn reserved_bytes(&self) -> usize {
        self.bytes + self.discarded_bytes + self.running * JOB_BYTES
    }

    pub(super) fn retire(&mut self, path: &PathBuf) {
        let entry = self.entries.get_mut(path).expect("tracked image");
        let old = std::mem::replace(&mut entry.state, State::Deferred);
        if let State::Ready(Ok(image)) = old {
            let bytes = image.as_bytes(0).map_or(0, <[u8]>::len);
            self.bytes -= bytes;
            self.discarded_bytes += bytes;
            self.discarded.push(image);
        }
        entry.blocked.extend(entry.leases.keys().copied());
        self.loaded.retain(|loaded| loaded != path);
    }

    pub(super) fn invalidate(&mut self, path: &PathBuf, version: Version, cx: &mut Context<Self>) {
        self.retire(path);
        let entry = self.entries.get_mut(path).expect("tracked image");
        entry.generation += 1;
        entry.version = Some(version);
        entry.state = State::Queued;
        Self::notify(entry, cx);
    }

    pub(super) fn pump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.prune_inactive();
        while self.running < MAX_JOBS {
            let Some(path) = self.entries.iter().find_map(|(path, entry)| {
                matches!(entry.state, State::Queued).then(|| path.clone())
            }) else {
                break;
            };
            if self.reserved_bytes() + JOB_BYTES > CACHE_BYTES {
                if self.reserved_bytes() - self.discarded_bytes + JOB_BYTES <= CACHE_BYTES {
                    break;
                }
                if let Some(oldest) = self.loaded.front().cloned() {
                    self.retire(&oldest);
                    Self::notify(&self.entries[&oldest], cx);
                    continue;
                }
                break;
            }
            let entry = self.entries.get_mut(&path).expect("queued image");
            entry.state = State::Loading;
            entry.workers += 1;
            let generation = entry.generation;
            self.running += 1;
            self.start_job(path, generation, cx);
        }
        if self.discarded.is_empty() || self.maintenance_scheduled {
            return;
        }
        self.maintenance_scheduled = true;
        let weak = cx.weak_entity();
        window.defer(cx, move |window, cx| {
            // SILENT-OK: Owner teardown separately retires all images if this weak handle expired.
            let _ = weak.update(cx, |cache, cx| {
                for image in cache.discarded.drain(..) {
                    // SILENT-OK: Pinned GPUI drop_image only removes atlas entries and returns Ok.
                    let _ = window.drop_image(image);
                }
                cache.discarded_bytes = 0;
                cache.maintenance_scheduled = false;
                cache.pump(window, cx);
            });
        });
    }
}
