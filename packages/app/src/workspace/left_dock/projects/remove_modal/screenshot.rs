//! Synthetic recovery choices for capture; no filesystem operation is started.

use super::*;
use gpui::Entity;
use std::path::PathBuf;

pub(in crate::workspace) fn open_for_shot(
    workspace: &Entity<Workspace>,
    delete_folder: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let workspace = workspace.downgrade();
    crate::workspace::dialog_helpers::open_form_modal(
        s::modal::remove_lane_title(),
        None,
        move |_, cx| RemoveWorktreeModal {
            focus_handle: cx.focus_handle(),
            target: LaneRef {
                project: 0,
                lane: 99,
            },
            target_label: "feature/SERVER-2200".into(),
            target_path: "/Users/demo/dev/apm-feature-SERVER-2200".into(),
            plan: RemoveWorktreePlan {
                path: PathBuf::from("/Users/demo/dev/apm-feature-SERVER-2200"),
                repo_root: PathBuf::from("/Users/demo/dev/apm"),
                lock_dir: None,
            },
            branch: Some("feature/SERVER-2200".into()),
            delete_branch_too: false,
            delete_folder,
            stage: Stage::Ready(LaneRemovalState::Unregistered),
            error: None,
            allow_force: false,
            submitting: false,
            workspace,
        },
        window,
        cx,
    );
}

#[cfg(test)]
mod tests {}
