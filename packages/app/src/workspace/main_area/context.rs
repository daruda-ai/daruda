use std::collections::HashMap;

use daruda_store::project::LaneRef;
use gpui::Task;

use super::pane::Pane;
use super::pane_menu::ResourceRightClick;
use super::pane_tree::{DropHalf, PaneId};
use super::tab_drop_plan::DropSlot;
use super::tab_strip::TabId;
use crate::workspace::LaneRuntime;
use crate::workspace::layout::ops::{DividerDrag, PopupMenuDeploy};

/// Runtime state of the main area — TabBar + PaneTree — for every lane.
///
/// Grouped out of `Workspace` so the tab/pane state has a clear
/// conceptual boundary and the migration path to a GPUI Entity is
/// straightforward (the struct becomes the Entity's state verbatim).
///
/// `runtimes` is the single store: every lane's runtime lives here,
/// keyed by `LaneRef`. The active lane is just the entry under
/// `Workspace::active` — switching lanes re-points that key, never
/// moves data. `Workspace::active_runtime()` / `active_runtime_mut()`
/// resolve the active entry, which is guaranteed present (seeded at
/// construction and on every `activate_lane`).
#[derive(Default)]
pub(in crate::workspace) struct MainAreaContext {
    /// Every lane's live runtime (tabs / panes / focus / history),
    /// keyed by `LaneRef`. The active lane is the entry under
    /// `Workspace::active`; there is no separate active/inactive split.
    pub runtimes: HashMap<LaneRef, LaneRuntime>,
    pub pending_resize: bool,
    /// Monotonic counter incremented every time a pane gains focus.
    /// Used by directional navigation (nav.rs) as the tie-breaker.
    pub activity_counter: HashMap<PaneId, u64>,
    pub activity_tick: u64,
    pub last_viewport: Option<(f32, f32)>,
    /// The width both tab groups are drawn in, as last measured.
    pub groups_width: f32,
    /// The divider between the tab groups being dragged.
    pub side_divider_drag: Option<super::side_ops::SideDividerDrag>,
    /// Active pane-divider drag. `None` = no drag in progress.
    pub drag_state: Option<DividerDrag>,
    /// The open right-click menu, if any (root-deployed) — see `PopupMenuDeploy`.
    pub popup_menu_deploy: Option<PopupMenuDeploy>,
    /// The resource link a right press landed on, until that press's menu
    /// takes it — see `ResourceRightClick`.
    pub resource_right_click: Option<ResourceRightClick>,
    /// The pane of a tab drawn alone over the area. Private: whether it
    /// zooms anything now is `Workspace::zoom_scope`, since a pane of
    /// another worktree or of a tab not shown zooms nothing.
    zoomed_pane_id: Option<PaneId>,
    /// Transient hover target while a Pane header is being dragged:
    /// the pane under the cursor and which half it would split. Single
    /// source of truth for the drop-overlay; `None` = no active hover.
    pub pane_drop_hover: Option<(PaneId, DropHalf)>,
    /// What a tab drag in flight has set — see [`TabDragState`].
    pub tab_drag: TabDragState,
}

/// The transient state of one tab drag. `Workspace::finish_tab_drag` is the
/// one place it is taken down.
#[derive(Default)]
pub(in crate::workspace) struct TabDragState {
    /// Tab armed to switch-on-hover, and the cancellable timer task backing
    /// it. Overwriting drops the previous `Task`, which cancels it (mirrors
    /// GPUI's own tooltip-delay mechanism).
    pub hover_switch: Option<(TabId, Task<()>)>,
    /// The slot of either strip the drag is over, tagged with the cell
    /// that set it so only that cell releases it again.
    pub slot: Option<DropSlot>,
    /// Tab that was shown before the drag's first hover preview, by id.
    /// Restored unless the drag committed a drop.
    pub preview_restore: Option<TabId>,
}

impl MainAreaContext {
    /// The pane with this id, in whichever lane holds it. Pane ids come from
    /// the window-wide `Workspace::alloc_id`, so at most one lane matches.
    pub(in crate::workspace) fn pane(&self, id: PaneId) -> Option<&Pane> {
        self.runtimes
            .values()
            .flat_map(|runtime| runtime.panes.iter())
            .find(|pane| pane.id == id)
    }

    /// Mutable counterpart of [`Self::pane`].
    pub(in crate::workspace) fn pane_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
        self.runtimes
            .values_mut()
            .flat_map(|runtime| runtime.panes.iter_mut())
            .find(|pane| pane.id == id)
    }
}

impl MainAreaContext {
    /// The pane asked to be drawn alone, whether or not it is on screen.
    pub(in crate::workspace) fn zoomed_pane(&self) -> Option<PaneId> {
        self.zoomed_pane_id
    }

    pub(in crate::workspace) fn set_zoomed_pane(&mut self, pane: Option<PaneId>) {
        self.zoomed_pane_id = pane;
    }

    /// Stand the zoom down, handing back what it was.
    pub(in crate::workspace) fn take_zoomed_pane(&mut self) -> Option<PaneId> {
        self.zoomed_pane_id.take()
    }
}
