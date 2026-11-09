//! Rendering artifacts derived from the conversation: the async-built ones the
//! reconcilers fill and `render/` reads, and the Markdown parses `render/` keeps.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::render::{
    DiffEditors, DiffStats, MermaidImages, OutputEditors, ResourceImages, ToolImages,
};

/// Async-built rendering artifacts derived from the conversation: read-only
/// diff editors, read-only verbatim tool-output editors, rasterized mermaid
/// diagrams, and decoded tool-output images. Each is filled by its own
/// reconciler in `reconcile/` and only read by `render/`; the exception is
/// `markdown_states`, which the render pass fills (see its field).
/// One field instead of many, with one [`Self::clear`] wiping all of them on a
/// session reset.
#[derive(Default)]
pub(crate) struct AssetCache {
    /// Read-only diff editor entities for tool-call file edits, keyed by
    /// `"{tool_call_id}#{diff_index}"`. Built by `reconcile_diff_editors`;
    /// `render` only embeds them, never creates one.
    pub(crate) diff_editors: DiffEditors,
    /// Fingerprint each `diff_editors` entry was built from. Lets
    /// `reconcile_diff_editors` detect a replaced diff (streaming growth from
    /// a partial snapshot) and rebuild instead of leaving it stale.
    pub(crate) diff_editor_sources: HashMap<String, u64>,
    /// Added/removed line counts per tool-call diff, same keys as
    /// `diff_editors`. Runtime cache; never serialized.
    pub(crate) diff_stats: DiffStats,
    /// Read-only editor entities for verbatim tool-output blocks, keyed by
    /// `"{tool_call_id}#{block_index}"`. Built by `reconcile_output_editors`;
    /// `render` only embeds them, never creates one.
    pub(crate) output_editors: OutputEditors,
    /// Fingerprint each `output_editors` entry was built from, so a streamed
    /// output that grew is rebuilt rather than left frozen on a partial
    /// snapshot.
    pub(crate) output_editor_sources: HashMap<String, u64>,
    /// Rendered mermaid diagrams by fence-source hash, filled async by
    /// `reconcile_mermaid`. Shared `Arc<Mutex<…>>` so gpui's texture cache
    /// hits instead of re-uploading the bitmap every frame.
    pub(crate) mermaid_images: MermaidImages,
    /// Source hashes with a rasterization currently spawned, so
    /// `reconcile_mermaid` doesn't re-spawn while one is still rendering.
    pub(crate) mermaid_inflight: HashSet<u64>,
    /// Decoded tool-output images by content hash. `Some` = decoded; `None` =
    /// cached decode failure (renders a label once, never re-spawned). Filled
    /// async by `reconcile_tool_images`.
    pub(crate) tool_images: ToolImages,
    /// Content hashes with a decode currently spawned, so
    /// `reconcile_tool_images` doesn't re-spawn one still decoding.
    pub(crate) tool_image_inflight: HashSet<u64>,
    /// Decoded local image resources by `"{tool_call_id}#{block_index}"`.
    pub(crate) resource_images: ResourceImages,
    /// Fingerprint of the resource URI + MIME currently represented by each
    /// cache entry, so a replaced streamed block invalidates the old bitmap.
    pub(crate) resource_image_sources: HashMap<String, u64>,
    /// Least-recently reconciled to most-recently reconciled resource keys.
    /// Keeps the decoded cache bounded even when a transcript contains many
    /// expanded image-producing tools.
    pub(crate) resource_image_order: VecDeque<String>,
    /// Fingerprint currently being read for each resource block. Storing the
    /// value as well as the key prevents a late old task overwriting a newer
    /// source at the same block index.
    pub(crate) resource_image_inflight: HashMap<String, u64>,
    /// Parsed Markdown bodies by element id. The one cache the render pass
    /// fills rather than a reconciler: it takes the place of GPUI keyed state,
    /// which drops a body's parse the first frame the virtualized list does not
    /// draw its row, so a long body scrolled back into view would lay out empty
    /// until a background reparse landed. Ids carry an item index or a tool-call
    /// id, the same identity the other caches key on; a replayed tail that
    /// reuses one hands its body the new text. Every body drawn stays parsed
    /// until a discarded conversation clears it with the rest.
    pub(crate) markdown_states: daruda_ui::MarkdownStates,
}

impl AssetCache {
    /// Wipe every cache in place — same `Arc`s, contents cleared — so a live
    /// render-side clone of the shared image caches (cloned fresh per render;
    /// see `render/blocks.rs`) stays attached rather than orphaned.
    pub(super) fn clear(&mut self) {
        self.diff_editors.clear();
        self.diff_editor_sources.clear();
        self.diff_stats.clear();
        self.output_editors.clear();
        self.output_editor_sources.clear();
        self.clear_mermaid();
        self.markdown_states.clear();
        self.tool_image_inflight.clear();
        if let Ok(mut m) = self.tool_images.lock() {
            m.clear();
        }
        self.resource_image_sources.clear();
        self.resource_image_order.clear();
        self.resource_image_inflight.clear();
        if let Ok(mut m) = self.resource_images.lock() {
            m.clear();
        }
    }

    pub(crate) fn clear_mermaid(&mut self) {
        self.mermaid_inflight.clear();
        if let Ok(mut m) = self.mermaid_images.lock() {
            m.clear();
        }
    }
}
