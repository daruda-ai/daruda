//! Pane-area file viewer — open / split / mode / scroll / load.

use std::path::PathBuf;

use daruda_store::project::LaneRef;
use gpui::{Context, Window, point, px};

use crate::lane::availability::LaneAvailability;
use crate::ui::diff_editor::{DiffColors, build_diff_editor_model};
use crate::workspace::Workspace;
use crate::workspace::main_area::file_view_pane::{
    DiffSource, FileOrigin, FileViewMode, PaneFileContent, PaneFileView,
};
use crate::workspace::main_area::open_plan::{GroupFacts, OpenFacts, OpenPlan, plan_open};
use crate::workspace::main_area::pane::FileContent;
use crate::workspace::main_area::pane_tree::PaneId;
use crate::workspace::main_area::side_group::{PanePlace, SideGroup, SideTab};
use crate::workspace::main_area::tab_ops::{OpenIntent, PaneEntry};

mod load;

fn line_to_editor_position(line: usize) -> gpui_component::input::Position {
    let row = line.saturating_sub(1);
    gpui_component::input::Position::new(u32::try_from(row).unwrap_or(u32::MAX), 0)
}

#[derive(Clone)]
struct FilePaneLoadRequest {
    pane_id: PaneId,
    owner: LaneRef,
    path: PathBuf,
    source: DiffSource,
    mode: FileViewMode,
    live_status: Option<char>,
}

impl FilePaneLoadRequest {
    fn from_view(pane_id: PaneId, owner: LaneRef, view: &PaneFileView) -> Self {
        Self {
            pane_id,
            owner,
            path: view.path.clone(),
            source: view.source.clone(),
            mode: view.view_mode,
            live_status: view.live_status,
        }
    }

    fn matches_view(&self, view: &PaneFileView) -> bool {
        view.path == self.path && view.source == self.source && view.view_mode == self.mode
    }
}

fn title_for_file_path(path: &std::path::Path) -> gpui::SharedString {
    path.file_name()
        .map(|n| gpui::SharedString::from(n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| {
            gpui::SharedString::from(crate::surface::strings::file_viewer::untitled_tab_title())
        })
}

fn markdown_raw_line_scroll_offset(line: usize, cx: &gpui::App) -> gpui::Point<gpui::Pixels> {
    let row = line.saturating_sub(1);
    let row_h = crate::ui::theme::editor_font_size(cx) * crate::ui::theme::editor_line_height(cx);
    point(
        px(crate::ui::theme::FILE_VIEWER_SCROLL_ORIGIN_X),
        px(-(row as f32 * row_h)),
    )
}

fn apply_pending_file_viewer_scroll(
    fc: &mut FileContent,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let Some(line) = fc.view.pending_scroll_line() else {
        return;
    };

    match &fc.view.content {
        PaneFileContent::LoadedRaw { .. } | PaneFileContent::LoadedDiff { .. } => {
            fc.view.clear_pending_scroll_line();
            let editor = fc.editor_state.clone();
            editor.update(cx, |state, cx| {
                state.set_cursor_position(line_to_editor_position(line), window, cx);
            });
        }
        PaneFileContent::LoadedMarkdown { .. } => {
            apply_pending_file_viewer_scroll_without_window(fc, cx);
        }
        PaneFileContent::Loading => {}
        PaneFileContent::Error(_) | PaneFileContent::Binary | PaneFileContent::Deleted => {
            fc.view.clear_pending_scroll_line();
        }
    }
}

fn apply_pending_file_viewer_scroll_without_window(
    fc: &mut FileContent,
    cx: &mut Context<Workspace>,
) {
    let Some(line) = fc.view.pending_scroll_line() else {
        return;
    };

    match &fc.view.content {
        PaneFileContent::LoadedMarkdown { raw_rows, .. }
            if fc.view.view_mode == FileViewMode::Raw =>
        {
            let line = line.min(raw_rows.len().max(1));
            fc.view.clear_pending_scroll_line();
            fc.scroll_handle
                .set_offset(markdown_raw_line_scroll_offset(line, cx));
            cx.notify();
        }
        PaneFileContent::Loading => {}
        _ => {
            fc.view.clear_pending_scroll_line();
        }
    }
}

/// A file to show: what `open_file_pane` settled before choosing its tab.
struct FileOpen {
    origin: FileOrigin,
    path: PathBuf,
    source: DiffSource,
    live_status: Option<char>,
    /// The mode asked for; the one shown depends on the file.
    mode: FileViewMode,
}

impl Workspace {
    fn file_content_mut_for_pane(&mut self, pane_id: PaneId) -> Option<&mut FileContent> {
        self.active_runtime_mut()
            .panes
            .iter_mut()
            .find(|p| p.id == pane_id)
            .and_then(|p| p.file_content_mut())
    }

    /// Record whether the tab at `place` is the replaceable scratch one.
    /// A preview claims the slot; anything else releases it, which is how a
    /// file the user committed to stops being something a later skim can take.
    ///
    /// Only for the open that actually *filled* the tab — see
    /// [`Self::release_preview_tab`] for why re-activating one must not claim.
    fn mark_preview_tab(&mut self, place: PanePlace, intent: OpenIntent) {
        if !intent.claims_preview_slot() {
            self.release_preview_tab(place);
            return;
        }
        if let Some(tab_id) = self.tab_id_at(place) {
            self.active_runtime_mut().preview_tab_id = Some(tab_id);
        }
    }

    /// Drop the scratch slot if the tab at `place` holds it.
    pub(in crate::workspace) fn release_preview_tab(&mut self, place: PanePlace) {
        let Some(tab_id) = self.tab_id_at(place) else {
            return;
        };
        let rt = self.active_runtime_mut();
        if rt.preview_tab_id == Some(tab_id) {
            rt.preview_tab_id = None;
        }
    }

    /// Walking into a pane is a commit: the tab holding it stops being the
    /// scratch one. Without this a row the arrow preview opened and the user
    /// then entered stays replaceable, and the next arrow key takes the file
    /// they had walked into.
    pub(in crate::workspace) fn release_preview_tab_for_pane(&mut self, pane_id: PaneId) {
        // `place_of` walks a tab's split tree, not just a one-leaf tab: a
        // pane can be split and collapsed back onto the same `TabEntry`, and
        // the slot has to be released for the tab that actually holds it.
        if let Some(place) = self.active_runtime().place_of(pane_id) {
            self.release_preview_tab(place);
        }
    }

    /// What the two tab groups are like now, for deciding where an open
    /// goes (`open_plan`).
    /// Whether the tab shown on the left holds a pane only the tabs take:
    /// a shell or a chat.
    fn work_shown(&self) -> bool {
        let runtime = self.active_runtime();
        runtime.tabs.active_tab().is_some_and(|tab| {
            tab.layout.pane_ids().iter().any(|id| {
                runtime
                    .panes
                    .iter()
                    .any(|pane| pane.id == *id && !pane.content.behavior().hosting().aside())
            })
        })
    }

    pub(in crate::workspace) fn group_facts(&self) -> GroupFacts {
        GroupFacts {
            keyboard_aside: self.keyboard_is_aside(),
            beside_setting: self.mirrors.file_viewer_open_beside,
            tabs: self.active_runtime().tabs.len(),
            work_shown: self.work_shown(),
            fits_both: SideGroup::fits_beside(
                self.main_area.groups_width,
                crate::ui::theme::TAB_GROUP_MIN_WIDTH,
            ),
        }
    }

    /// Show the tab at `place`, moving into its pane when `entry` enters.
    fn show_tab_at(
        &mut self,
        place: PanePlace,
        entry: PaneEntry,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        match (place, entry) {
            (PanePlace::Tab(index), _) => self.activate_tab_as(index, entry, window, cx),
            (PanePlace::Side(index), PaneEntry::Enter) => self.activate_side_tab(index, window, cx),
            (PanePlace::Side(index), PaneEntry::Preview) => self.show_side_tab(index, window, cx),
        }
    }

    /// Select a file in the Git Changes view: open the pane-area file viewer
    /// in a new tab (or activate the existing tab if the file is already open).
    ///
    /// `intent` is never [`OpenIntent::Enter`]: every way to reach this is a
    /// panel row, and a panel row that moved the user into the pane would take
    /// its own arrow keys with it. The caller still chooses between resting on
    /// the row ([`OpenIntent::Preview`]) and picking it ([`OpenIntent::Commit`]).
    pub(in crate::workspace) fn open_git_file_diff(
        &mut self,
        target: LaneRef,
        path: PathBuf,
        source: DiffSource,
        intent: OpenIntent,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        debug_assert_ne!(intent, OpenIntent::Enter, "a git row never enters the pane");
        self.open_pane_file_view(
            target,
            path,
            source,
            FileViewMode::Changes,
            intent,
            window,
            cx,
        );
    }

    /// Open the pane-area file viewer for `path`.
    ///
    /// Behaviour depends on `file_viewer_preview_tab` config:
    ///
    /// **Preview-tab mode** (default, `preview_tab = true`):
    ///   - Same file already open → activate its tab (no duplicate).
    ///   - Any other file-viewer tab exists → replace its content in place
    ///     and activate it.
    ///   - No file-viewer tab → open a new tab.
    ///
    /// **Multi-tab mode** (`preview_tab = false`):
    ///   - Same file already open → activate its tab (no duplicate).
    ///   - Otherwise → always open a new tab.
    ///
    /// `initial_mode` selects Raw (Files view) or Changes (Git view);
    /// Markdown files open in Preview by default when Raw was requested.
    ///
    /// The pane's `live_status` is derived here via
    /// [`Self::git_status_for_path`], never supplied by the caller.
    ///
    /// `intent` says who ends up focused and whether the tab is replaceable;
    /// see [`OpenIntent`].
    // Every argument is an independent axis of what to open; bundling them
    // into a struct would tax all eight call sites to satisfy the lint.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::workspace) fn open_pane_file_view(
        &mut self,
        target: LaneRef,
        path: PathBuf,
        source: DiffSource,
        initial_mode: FileViewMode,
        intent: OpenIntent,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        self.open_file_pane(
            target,
            FileOrigin::Lane,
            path,
            source,
            initial_mode,
            intent,
            window,
            cx,
        );
    }

    /// Open a file an agent pointed at: in the lane that owns it, like any
    /// lane file — or, when no lane holds it, for reference in the lane on
    /// screen, read-only. An agent may name any path on disk.
    pub(in crate::workspace) fn open_linked_file(
        &mut self,
        path: PathBuf,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        match self.projects.lane_owning(&path) {
            Some(owner) => self.open_pane_file_view(
                owner,
                path,
                DiffSource::WorkingTree,
                FileViewMode::Raw,
                OpenIntent::Enter,
                window,
                cx,
            ),
            None => self.open_reference_file(path, window, cx),
        }
    }

    /// Open `path` read-only in the lane on screen, for a panel that only
    /// consults lanes (Skills, Tools). The file may live anywhere on disk,
    /// so no lane is looked up and none is moved to.
    pub(in crate::workspace) fn open_reference_file(
        &mut self,
        path: PathBuf,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        self.open_file_pane(
            self.active,
            FileOrigin::Reference,
            path,
            DiffSource::WorkingTree,
            FileViewMode::Raw,
            OpenIntent::Enter,
            window,
            cx,
        );
    }

    /// The one place a file pane is opened. A pane lives in the runtime of
    /// the lane it belongs to, so a user's open of another lane's file moves
    /// there first — the pane and the lane its content is read against are
    /// then the same by construction.
    #[allow(clippy::too_many_arguments)]
    fn open_file_pane(
        &mut self,
        target: LaneRef,
        origin: FileOrigin,
        path: PathBuf,
        source: DiffSource,
        initial_mode: FileViewMode,
        intent: OpenIntent,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        // A lane whose path is gone cannot show a file, and is not moved to.
        let present = |ws: &Self| {
            ws.lane_for(target)
                .filter(|lane| lane.availability == LaneAvailability::Present)
                .map(|lane| lane.path.clone())
        };
        // A pane is its lane's content only when the file is inside the lane;
        // anything else is read for reference. Decided here, once, so whether
        // a pane edits and whether it saves are one answer. A reference file
        // belongs to no lane, so it needs none — not even in an empty window.
        let origin = match origin {
            FileOrigin::Lane => {
                let Some(lane_root) = present(self) else {
                    return;
                };
                if daruda_core::path::is_within(&path, &lane_root) {
                    FileOrigin::Lane
                } else {
                    FileOrigin::Reference
                }
            }
            FileOrigin::Reference => FileOrigin::Reference,
        };
        if target != self.active {
            self.activate_lane(target, window, cx);
            // Activation re-checks the path; a lane found gone opens nothing.
            if target != self.active || present(self).is_none() {
                return;
            }
        }
        let owner = target;
        let live_status = self.git_status_for_path(owner, &path, &source);

        let open = FileOpen {
            origin,
            path,
            source,
            live_status,
            mode: initial_mode,
        };
        let facts = OpenFacts {
            groups: self.group_facts(),
            existing: self.existing_file_tab(&open.path, &open.source),
            scratch: self.preview_slot(cx),
        };
        let pane_id = match plan_open(intent, &facts) {
            OpenPlan::Show((place, pane_id)) => {
                self.show_open_file(place, pane_id, &open, intent, window, cx);
                return;
            }
            // Preview-tab mode: the scratch tab takes the file.
            OpenPlan::Refill((place, pane_id)) => {
                if !self.refill_scratch_tab(place, pane_id, &open, intent, window, cx) {
                    return;
                }
                pane_id
            }
            OpenPlan::New { beside } => self.push_file_tab(&open, beside, intent, window, cx),
            OpenPlan::Replace((_, scratch)) => {
                self.close_pane_by_id(scratch, window, cx);
                // Asked once the scratch tab is gone: the close took a tab
                // only if the pane was alone in it, and the keyboard with
                // it only if it was there.
                let beside = self.group_facts().goes_beside(intent.pane_entry());
                self.push_file_tab(&open, beside, intent, window, cx)
            }
        };
        // Selection moved — the dock row picks up its selected background.
        self.invalidate_visible_files_cache(owner);
        cx.notify();
        if let Some(fc) = self.file_content_mut_for_pane(pane_id) {
            let request = FilePaneLoadRequest::from_view(pane_id, owner, &fc.view);
            self.load_pane_file_content(request, cx);
        }
    }

    /// The tab already showing `path` as `source` reads it, in either group.
    fn existing_file_tab(
        &self,
        path: &std::path::Path,
        source: &DiffSource,
    ) -> Option<(PanePlace, PaneId)> {
        self.side_tab_showing(path, source)
            .map(|index| {
                (
                    PanePlace::Side(index),
                    self.active_runtime().side.tabs()[index].pane,
                )
            })
            .or_else(|| {
                self.find_existing_file_tab(path, source)
                    .map(|(index, pane)| (PanePlace::Tab(index), pane))
            })
    }

    /// Show the tab at `place`, which already holds the file `open` asks for.
    fn show_open_file(
        &mut self,
        place: PanePlace,
        pane_id: PaneId,
        open: &FileOpen,
        intent: OpenIntent,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        // Re-stamp the tab being reused. It was opened against an older
        // the lane's cached status, and the toolbar's mode strip reads
        // `status()` to decide whether Changes is offered at all.
        if let Some(fc) = self.file_content_mut_for_pane(pane_id)
            && (fc.view.live_status != open.live_status || fc.view.origin != open.origin)
        {
            fc.view.live_status = open.live_status;
            // A file first opened for reference may since have become a
            // lane's own, or the reverse.
            fc.view.origin = open.origin;
            cx.notify();
        }
        // Re-activating a tab that already holds this file: a deliberate
        // open releases the slot, but a preview must not claim it. Skimming
        // back across a row the user opened with Enter would otherwise make
        // it replaceable again (zed's `set_up_existing_item` likewise only
        // ever un-previews).
        if !intent.claims_preview_slot() {
            self.release_preview_tab(place);
        }
        self.show_tab_at(place, intent.pane_entry(), window, cx);
    }

    /// Put `open`'s file in the scratch tab at `place`, keeping the pane:
    /// its scroll handle, search input, focus handle and subscription.
    /// `false` when the pane is no longer a file's.
    fn refill_scratch_tab(
        &mut self,
        place: PanePlace,
        pane_id: PaneId,
        open: &FileOpen,
        intent: OpenIntent,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let mode = FileViewMode::effective_for_path(open.mode, &open.path);
        let Some(fc) = self.file_content_mut_for_pane(pane_id) else {
            return false;
        };
        // The reused pane's images belong to the file it is leaving.
        fc.release_images(Some(&mut *window), cx);
        fc.view.replace_with_loading(
            open.origin,
            open.path.clone(),
            open.source.clone(),
            open.live_status,
            mode,
        );
        fc.scroll_handle = gpui::ScrollHandle::new();
        fc.cached_title = title_for_file_path(&open.path);
        fc.search_input
            .update(cx, |inp, cx_state| inp.set_value("", window, cx_state));
        // Clear the reused tab's user label (it was set for the old file).
        let runtime = self.active_runtime_mut();
        match place {
            PanePlace::Tab(index) => {
                if let Some(tab) = runtime.tabs.tab_mut(index) {
                    tab.user_label = None;
                }
            }
            PanePlace::Side(index) => runtime.side.clear_label(index),
        }
        let entry = intent.pane_entry();
        self.mark_preview_tab(place, intent);
        self.show_tab_at(place, entry, window, cx);
        if entry == PaneEntry::Enter {
            self.focus_pane(pane_id, window, cx);
        }
        true
    }

    /// Open `open`'s file in a new tab: a side tab when `beside`.
    fn push_file_tab(
        &mut self,
        open: &FileOpen,
        beside: bool,
        intent: OpenIntent,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) -> PaneId {
        let entry = intent.pane_entry();
        let pane = self.create_file_pane(
            open.origin,
            open.path.clone(),
            open.source.clone(),
            open.live_status,
            FileViewMode::effective_for_path(open.mode, &open.path),
            window,
            cx,
        );
        let pane_id = pane.id;
        let tab_id = self.alloc_tab_id();
        self.active_runtime_mut().panes.push(pane);
        if beside {
            let index = self.active_runtime_mut().side.open(SideTab {
                id: tab_id,
                pane: pane_id,
                user_label: None,
            });
            self.mark_preview_tab(PanePlace::Side(index), intent);
            // The side tabs may have just come into view.
            self.resize_all_tabs(window, cx);
            self.bump_activity(pane_id);
            if entry == PaneEntry::Enter {
                self.focus_side_group(window, cx);
            }
            return pane_id;
        }
        let runtime = self.active_runtime_mut();
        let last_tab = runtime
            .tabs
            .open(crate::workspace::main_area::pane::TabEntry {
                id: tab_id,
                layout: crate::workspace::main_area::pane_tree::PaneLayout::Pane(pane_id),
                last_focused_pane: pane_id,
                user_label: None,
            });
        self.mark_preview_tab(PanePlace::Tab(last_tab), intent);
        self.set_focused_pane(pane_id, window, cx);
        self.bump_activity(pane_id);
        if entry == PaneEntry::Enter {
            self.focus_pane(pane_id, window, cx);
        }
        pane_id
    }

    /// Switch a file pane between Raw / Preview / Changes mode.
    /// For a Markdown file switching between Raw and Preview, the content is
    /// already loaded in both representations — skip the reload.
    pub(in crate::workspace) fn set_file_view_mode(
        &mut self,
        mode: FileViewMode,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        let pane_id = self.keyboard_pane();
        self.set_file_view_mode_for_pane(pane_id, mode, window, cx);
    }

    /// `window` is only here to release the outgoing mode's GPU images — see
    /// [`crate::workspace::main_area::pane::FileContent::release_images`].
    pub(in crate::workspace) fn set_file_view_mode_for_pane(
        &mut self,
        pane_id: PaneId,
        mode: FileViewMode,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        // Apply the mutation in an inner scope so the focused-pane borrow
        // releases before we call `load_pane_file_content` (which reborrows
        // self for the spawn).
        let Some(owner) = self.lane_ref_for_pane(pane_id) else {
            return;
        };
        let load_request = {
            let Some(fc) = self.file_content_mut_for_pane(pane_id) else {
                return;
            };
            let Some(needs_reload) = fc.begin_mode_change(mode, Some(&mut *window), cx) else {
                return;
            };
            fc.scroll_handle = gpui::ScrollHandle::new();

            if needs_reload {
                Some(FilePaneLoadRequest::from_view(pane_id, owner, &fc.view))
            } else {
                None
            }
        };
        cx.notify();

        if let Some(request) = load_request {
            self.load_pane_file_content(request, cx);
        }
    }

    /// Toggle whether context lines are hidden in Changes (diff) mode.
    pub(in crate::workspace) fn toggle_hide_unchanged_for_pane(
        &mut self,
        pane_id: PaneId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(fc) = self.file_content_mut_for_pane(pane_id) else {
            return;
        };
        let needs_editor_rebuild = fc.view.toggle_hide_unchanged();
        let rebuild = needs_editor_rebuild.then(|| {
            let surface = crate::ui::theme::PaneSurfaceTokens::file_viewer(cx);
            cx.try_global::<crate::ui::theme::DarudaTheme>().map(|t| {
                build_diff_editor_model(
                    fc.view.active_rows(),
                    &DiffColors::from_file_viewer_surface(t, surface),
                    &crate::ui::theme::syntax_theme_of(
                        crate::ui::theme::active_syntax_palette(cx),
                        surface.syntax_is_light,
                    ),
                    true,
                )
            })
        });
        let editor = fc.editor_state.clone();
        cx.notify();
        if let Some(Some(model)) = rebuild {
            // The caller (toolbar mouse-down) already holds this window's live
            // `&mut Window` — update the editor entity directly with it rather
            // than going through `configure_file_editor`'s fresh
            // `WindowRegistry` + `cx.update_window` lookup. That lookup exists
            // for the async load-completion path, which has no live `Window`
            // in scope; calling it here, nested inside the same window's
            // active mouse-down dispatch, re-enters `cx.update_window` on a
            // window already checked out and silently fails ("window not
            // found", logged as a Warning) — the flag flips and the toolbar
            // repaints, but the editor's `set_value` never runs, so the diff
            // content never visually filters.
            editor.update(cx, move |state, cx_s| {
                state.set_value(model.text, window, cx_s);
                state.set_readonly(true, cx_s);
                state.set_line_decorations(model.decorations, cx_s);
                state.set_highlight_override(Some(model.highlights), cx_s);
            });
        }
    }

    /// Close the focused file pane's tab (the file viewer is its own
    /// `Pane` post-Plan-B, so closing it goes through the normal pane
    /// close path). No-op when the focused pane is a terminal.
    pub(in crate::workspace) fn close_focused_file_pane(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        let id = self.keyboard_pane();
        let is_file = self
            .active_runtime()
            .panes
            .iter()
            .any(|p| p.id == id && p.file_content().is_some());
        if !is_file {
            return;
        }
        // Through the prompting close, so an unsaved buffer is asked about.
        self.close_focused_pane(window, cx);
    }

    /// Scroll the focused file pane's body so the currently focused
    /// search match is visible. Does nothing when no match is focused,
    /// the focused pane is not a file viewer, or the viewer is in
    /// Preview mode (blocks have variable heights).
    pub(in crate::workspace) fn scroll_file_viewer_to_focused_match(&mut self, cx: &gpui::App) {
        let Some(fc) = self.focused_file_content() else {
            return;
        };
        let fv = &fc.view;
        if fv.view_mode == FileViewMode::Preview {
            return;
        }
        let Some(row) = fv.search_focused_row() else {
            return;
        };
        let line_h =
            crate::ui::theme::editor_font_size(cx) * crate::ui::theme::editor_line_height(cx);
        let target_y = row as f32 * line_h;
        let viewport_h: f32 = fc.scroll_handle.bounds().size.height.into();
        let current_y: f32 = (-fc.scroll_handle.offset().y).into();
        let visible_bottom = current_y + viewport_h;
        if target_y < current_y || target_y + line_h > visible_bottom {
            use gpui::{Point, px};
            let origin_x = crate::ui::theme::FILE_VIEWER_SCROLL_ORIGIN_X;
            fc.scroll_handle.set_offset(Point {
                x: px(origin_x),
                y: px(-target_y),
            });
        }
    }

    /// Reveal a 1-based source line in the focused file viewer. If the file is
    /// still loading, store the target and apply it when the load completes.
    pub(in crate::workspace) fn scroll_focused_file_viewer_to_line(
        &mut self,
        line: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if line == 0 {
            return;
        }
        let Some(fc) = self.focused_file_content_mut() else {
            return;
        };
        fc.view.set_pending_scroll_line(line);
        apply_pending_file_viewer_scroll(fc, window, cx);
    }
}
