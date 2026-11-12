//! Visible resource-image geometry and deferred cache maintenance.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use gpui::{AppContext as _, Bounds, Context, Pixels, Window};

use super::AgentChatView;
use crate::reconcile::ReconcileScope;

#[derive(Default)]
struct ViewportFrame {
    bounds: Bounds<Pixels>,
    keys: HashSet<String>,
}

/// Prepaint only records geometry here. The deferred view operation consumes
/// the completed frame after all resource cards have recorded their bounds.
#[derive(Clone, Default)]
pub(crate) struct ResourceImageViewport(Arc<Mutex<ViewportFrame>>);

impl ResourceImageViewport {
    pub(crate) fn begin_frame(&self, bounds: Bounds<Pixels>) {
        let mut frame = self.0.lock().unwrap();
        frame.bounds = bounds;
        frame.keys.clear();
    }

    pub(crate) fn record(&self, key: &str, bounds: Bounds<Pixels>) {
        let mut frame = self.0.lock().unwrap();
        if bounds.intersects(&frame.bounds) && bounds.size.height > gpui::px(0.) {
            frame.keys.insert(key.to_owned());
        }
    }

    pub(super) fn keys(&self) -> HashSet<String> {
        self.0.lock().unwrap().keys.clone()
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ResourceImageDimensions {
    pub(crate) width: f32,
    pub(crate) aspect_ratio: f32,
}

impl AgentChatView {
    pub(crate) fn sync_resource_image_viewport(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let weak = cx.entity().downgrade();
        let app = cx.to_async();
        let handle = window.window_handle();
        self.assets
            .resource_images
            .install_maintenance(move |orphaned| {
                let weak = weak.clone();
                let app_for_task = app.clone();
                app.foreground_executor()
                    .spawn(async move {
                        app_for_task.update(|cx| {
                            cx.defer(move |cx| {
                                if let Some(image) = orphaned {
                                    // SILENT-OK: a closed window no longer owns an atlas tile.
                                    let _ = cx.update_window(handle, |_, window, _| {
                                        drop_resource_texture(window, image)
                                    });
                                }
                                // SILENT-OK: consumer outlived its chat pane; no admission remains.
                                let _ = weak.update(cx, |_, cx| cx.notify());
                            })
                        });
                    })
                    .detach();
            });
        let changed = self.assets.resource_images.sync_viewport();
        let released = self.release_resource_images(window);
        if !changed && !released {
            return;
        }
        self.assets.resource_images.retry_budget_failures();
        self.reconcile_resource_images(&ReconcileScope::All, cx);
        if self.release_resource_images(window) {
            self.assets.resource_images.retry_budget_failures();
            self.reconcile_resource_images(&ReconcileScope::All, cx);
        }
        if changed {
            cx.notify();
        }
    }

    fn release_resource_images(&mut self, window: &mut Window) -> bool {
        let images = self.assets.resource_images.take_releasable();
        let released = !images.is_empty();
        for image in images {
            drop_resource_texture(window, image);
        }
        released
    }
}

fn drop_resource_texture(window: &mut Window, image: daruda_ui::CachedImage) {
    if let Err(error) = window.drop_image(image.render_image()) {
        daruda_store::observability::log_writer::LogWriter::log(
            daruda_store::observability::error_report::ErrorReport::new(
                "Unable to release resource image texture",
            )
            .from_error::<dyn std::error::Error>(error.as_ref())
            .at(file!(), line!())
            .build(),
        );
    }
}

#[cfg(test)]
mod tests;
