//! Weak background workers: all file inspection and raster decoding stays off UI.

#[cfg(test)]
mod tests;

use std::time::Duration;

use super::*;

const POLL_INTERVAL: Duration = Duration::from_millis(500);

fn decode(path: &std::path::Path) -> (Version, Version, ImageResult) {
    let before = Version::read(path);
    let result = daruda_content::local_image::load(path)
        .map_err(|error| ImageCacheError::Asset(error.to_string().into()))
        .and_then(|raster| {
            let buffer = image::RgbaImage::from_raw(raster.width, raster.height, raster.bgra)
                .ok_or_else(|| ImageCacheError::Asset("invalid decoded image buffer".into()))?;
            Ok(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
        });
    (before, Version::read(path), result)
}

impl LocalImageCache {
    pub(super) fn start_job(&self, path: PathBuf, generation: u64, cx: &mut Context<Self>) {
        let window = self.window;
        let worker_path = path.clone();
        let job = cx
            .background_executor()
            .spawn(async move { decode(&worker_path) });
        cx.spawn(async move |weak, cx| {
            let (before, after, result) = job.await;
            // SILENT-OK: A closed window has no consumers or retained atlas to update.
            let _ = window.update(cx, |_, window, cx| {
                // SILENT-OK: Dropped owners discard worker results without retaining their raster.
                let _ = weak.update(cx, |cache, cx| {
                    cache.running -= 1;
                    let entry = cache.entries.get_mut(&path).expect("running image");
                    entry.workers -= 1;
                    if entry.generation == generation {
                        if before != after {
                            entry.generation += 1;
                            entry.version = Some(after);
                            entry.state = State::Queued;
                        } else {
                            entry.version = Some(after);
                            if let Ok(image) = &result {
                                cache.bytes += image.as_bytes(0).map_or(0, <[u8]>::len);
                                cache.loaded.push_back(path.clone());
                            }
                            entry.state = State::Ready(result);
                        }
                        Self::notify(entry, cx);
                    }
                    cache.pump(window, cx);
                });
            });
        })
        .detach();
    }

    pub(super) fn start_poller(&mut self, cx: &mut Context<Self>) {
        if self.poller.is_some() {
            return;
        }
        let window = self.window;
        self.poller = Some(cx.spawn(async move |weak, cx| {
            loop {
                cx.background_executor().timer(POLL_INTERVAL).await;
                let Ok(paths) = weak.update(cx, |cache, _| {
                    cache
                        .entries
                        .iter()
                        .filter(|(_, entry)| entry.should_poll())
                        .map(|(path, entry)| {
                            (path.clone(), entry.generation, entry.version.clone())
                        })
                        .collect::<Vec<_>>()
                }) else {
                    break;
                };
                let versions = cx
                    .background_executor()
                    .spawn(async move {
                        paths
                            .into_iter()
                            .map(|(path, generation, old)| {
                                let version = Version::read(&path);
                                (path, generation, old, version)
                            })
                            .collect::<Vec<_>>()
                    })
                    .await;
                match window.update(cx, |_, window, cx| {
                    weak.update(cx, |cache, cx| {
                        for (path, generation, old, version) in versions {
                            let Some(entry) = cache.entries.get(&path) else {
                                continue;
                            };
                            if entry.should_poll()
                                && generation == entry.generation
                                && entry.version == old
                                && old.is_some()
                                && old.as_ref() != Some(&version)
                            {
                                cache.invalidate(&path, version, cx);
                            }
                        }
                        cache.pump(window, cx);
                    })
                }) {
                    Ok(Ok(())) => {}
                    // SILENT-OK: Owner teardown or window closure leaves no consumers to poll.
                    Ok(Err(_)) | Err(_) => break,
                }
            }
        }));
    }
}
