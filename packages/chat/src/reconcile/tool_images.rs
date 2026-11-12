//! Tool-output image decoding: inline `Image` blocks and bounded
//! previews of local-image resource links.

use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use daruda_acp::{ChatItem, ToolOutputBlock};
use daruda_content::link_target;
use daruda_content::visual;
use daruda_ui::CachedImage;
use gpui::Context;

use super::super::agent_chat_helpers::{TurnBoundary, tool_image_key};
use super::super::output_editor::output_editor_key;
use super::super::view::AgentChatView;
use super::{ReconcileScope, output_content};

/// Local resource previews are a fallback for adapter-emitted file links, not
/// a general URI fetcher. Keep reads bounded before bytes reach an image
/// decoder; decoded allocations are bounded separately in
/// `visual::decode_image_bounded`.
#[cfg(test)]
pub(super) use daruda_content::local_image::MAX_BYTES as MAX_RESOURCE_IMAGE_BYTES;
/// Resolved by the same rules the resource link's click uses, so the preview
/// and the opener cannot disagree about which file — or whether an image.
pub(super) fn resource_image_path(
    uri: &str,
    mime: Option<&str>,
    cwd: Option<&Path>,
) -> Option<PathBuf> {
    let path = link_target::resource_path(uri, cwd)?;
    link_target::is_image_resource(&path, mime).then_some(path)
}

fn resource_image_source(uri: &str, mime: Option<&str>) -> u64 {
    let mut hasher = DefaultHasher::new();
    uri.hash(&mut hasher);
    mime.hash(&mut hasher);
    hasher.finish()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResourceImageError {
    Read,
    NotFile,
    TooLarge,
    DecodeLimit,
    Decode,
    Render,
    Budget,
}

impl std::fmt::Display for ResourceImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Read => "unable to read image file",
            Self::NotFile => "image path is not a regular file",
            Self::TooLarge => "image exceeds the encoded size limit",
            Self::DecodeLimit => "image exceeds the dimension or memory limit",
            Self::Decode => "unsupported or invalid image data",
            Self::Render => "unable to prepare image for rendering",
            Self::Budget => "resource image memory budget is full",
        })
    }
}

pub(super) fn load_resource_image(path: &Path) -> Result<visual::RasterImage, ResourceImageError> {
    use daruda_content::local_image::LocalImageError;
    daruda_content::local_image::load(path).map_err(|error| match error {
        LocalImageError::Read => ResourceImageError::Read,
        LocalImageError::NotFile => ResourceImageError::NotFile,
        LocalImageError::TooLarge => ResourceImageError::TooLarge,
        LocalImageError::DecodeLimit => ResourceImageError::DecodeLimit,
        LocalImageError::Decode => ResourceImageError::Decode,
    })
}

impl AgentChatView {
    /// Decode every inline `Image` block and eligible local-image
    /// `ResourceLink` that does not yet have a cached bitmap. Mirrors
    /// [`Self::reconcile_mermaid`]'s collect-then-spawn shape, minus the `dark`
    /// theming (tool images are rendered as-is, not re-themed). Local resource
    /// files are read only after the call settles, on the background executor.
    ///
    /// Failures are cached so malformed content is not retried forever. Inline
    /// images show their media descriptor; resource images show a reason and
    /// retain their original link.
    pub(crate) fn reconcile_tool_images(&mut self, scope: &ReconcileScope, cx: &mut Context<Self>) {
        self.reconcile_inline_images(scope, cx);
        self.reconcile_resource_images(scope, cx);
    }

    fn reconcile_inline_images(&mut self, scope: &ReconcileScope, cx: &mut Context<Self>) {
        // Collect the not-yet-cached, not-in-flight images first; the spawn
        // re-enters the view, which can't happen while the `items` borrow is
        // live.
        let mut pending: Vec<(u64, String)> = Vec::new();
        for item in &self.items {
            let Some((id, output)) = output_content(item) else {
                continue;
            };
            if !scope.covers(&id) {
                continue;
            }
            for block in output {
                let ToolOutputBlock::Image { data, .. } = block else {
                    continue;
                };
                let key = tool_image_key(data);
                if self.assets.tool_images.lock().unwrap().contains_key(&key)
                    || self.assets.tool_image_inflight.contains(&key)
                    || pending.iter().any(|(k, _)| *k == key)
                {
                    continue;
                }
                pending.push((key, data.clone()));
            }
        }

        // Mark all pending keys in-flight before spawning so a second event
        // arriving before any task resolves doesn't re-spawn the same image.
        for (key, _) in &pending {
            self.assets.tool_image_inflight.insert(*key);
        }

        for (key, data) in pending {
            cx.spawn(async move |this, cx| {
                let raster = cx
                    .background_executor()
                    .spawn(async move {
                        // The decoder is a third-party crate over untrusted
                        // agent-supplied bytes; guard against a panic on
                        // malformed input so one bad payload can't take the
                        // executor down — on panic / error we drop it (cached
                        // as a failure below) instead of retrying.
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            use base64::Engine as _;
                            base64::engine::general_purpose::STANDARD
                                .decode(&data)
                                .ok()
                                .and_then(|bytes| visual::decode_image(&bytes).ok())
                        }))
                        .ok()
                        .flatten()
                    })
                    .await;
                // SILENT-OK: view/window dropped before the decode resolved — nothing left to cache it on.
                let _ = this.update(cx, |view, cx| {
                    view.assets.tool_image_inflight.remove(&key);
                    // Convert the raster to a GPU-ready image here (main
                    // thread), same as `reconcile_mermaid` — once, so the
                    // render hook clones the same `CachedImage` each frame.
                    // `None` (decode failed, or the conversion itself failed)
                    // is cached too, so a malformed payload renders a failure
                    // label once instead of retrying forever.
                    let cached = raster.and_then(CachedImage::from_raster);
                    view.assets.tool_images.lock().unwrap().insert(key, cached);
                    // The block just resolved from a pending placeholder to a
                    // decoded image (or a failure label), so its cached height
                    // is stale — remeasure before repainting or it clips.
                    // Index is unknown here, so this is a full remeasure, but
                    // it's one-shot per landed decode.
                    view.resync_all_row_heights("tool-image");
                    cx.notify();
                });
            })
            .detach();
        }
    }

    pub(crate) fn reconcile_resource_images(
        &mut self,
        scope: &ReconcileScope,
        cx: &mut Context<Self>,
    ) {
        let local_cwd = self
            .cwd
            .as_ref()
            .and_then(daruda_store::project::PaneCwd::as_local)
            .map(Path::to_path_buf);
        let mut resource_live = HashSet::new();
        let mut resource_candidates: Vec<(String, u64, PathBuf)> = Vec::new();
        if !matches!(&self.cwd, Some(daruda_store::project::PaneCwd::Remote(_))) {
            let boundary = TurnBoundary::of(&self.items);
            for (item_ix, item) in self.items.iter().enumerate() {
                let Some((id, output)) = output_content(item) else {
                    continue;
                };
                if !scope.covers(&id)
                    || matches!(item, ChatItem::ToolCall(tc) if tc.status.is_live())
                    || !self.tool_body_on_screen(item_ix, item, boundary)
                {
                    continue;
                }
                for (ix, block) in output.iter().enumerate() {
                    let ToolOutputBlock::ResourceLink { uri, mime, .. } = block else {
                        continue;
                    };
                    let Some(path) =
                        resource_image_path(uri, mime.as_deref(), local_cwd.as_deref())
                    else {
                        continue;
                    };
                    let key = output_editor_key(&id, ix);
                    let source = resource_image_source(uri, mime.as_deref());
                    resource_candidates.push((key, source, path));
                }
            }
        }

        resource_live.extend(resource_candidates.iter().map(|(key, ..)| key.clone()));

        self.assets
            .resource_images
            .reconcile_sources(&resource_live, scope);
        let resource_pending: Vec<_> = resource_candidates
            .into_iter()
            .filter(|(key, source, _)| self.assets.resource_images.wants(key, *source))
            .collect();
        if self.assets.resource_images.slots() == 0
            && self.assets.resource_images.inflight_count() == 0
            && !resource_pending.is_empty()
        {
            // Frame-pinned discards release once the next paint drops their
            // listeners; that release wakes maintenance, which retries admission.
            if self.assets.resource_images.prepare_admission() {
                cx.notify();
                return;
            }
            for (key, ..) in resource_pending {
                self.assets
                    .resource_images
                    .cache(key, Err(ResourceImageError::Budget));
            }
            self.resync_all_row_heights("resource-image-budget");
            cx.notify();
            return;
        }
        for (key, source, path) in resource_pending {
            let Some(job) = self.assets.resource_images.reserve(key, source) else {
                break;
            };
            cx.spawn(async move |this, cx| {
                let raster = cx
                    .background_executor()
                    .spawn(async move {
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            load_resource_image(&path)
                        }))
                        .unwrap_or(Err(ResourceImageError::Decode))
                    })
                    .await;
                // SILENT-OK: view dropped before its decoder resolved.
                let _ = this.update(cx, |view, cx| {
                    let cached = raster.and_then(|raster| {
                        CachedImage::from_raster(raster).ok_or(ResourceImageError::Render)
                    });
                    if view.assets.resource_images.complete(job, cached) {
                        view.resync_all_row_heights("resource-image");
                    }
                    cx.notify();
                    view.reconcile_resource_images(&ReconcileScope::All, cx);
                });
            })
            .detach();
        }
    }
}
