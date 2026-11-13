//! Remove a worktree, recover stale entries, and retry branch-only failures.

use crate::lane::git::LaneRemovalState;
use crate::surface::strings as s;
use crate::ui::WindowExt as _;
use crate::workspace::lane_ops::RemoveWorktreePlan;
use crate::workspace::{ModalView, Workspace};
use daruda_store::project::LaneRef;
use gpui::{App, Context, FocusHandle, Focusable, KeyDownEvent, SharedString, WeakEntity, Window};

mod operation;
mod render;
#[cfg(feature = "screenshot")]
mod screenshot;
#[cfg(feature = "screenshot")]
pub(in crate::workspace) use screenshot::open_for_shot;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Inspecting,
    Ready(LaneRemovalState),
    InspectionFailed,
    Branch,
}

pub struct RemoveWorktreeModal {
    focus_handle: FocusHandle,
    target: LaneRef,
    target_label: SharedString,
    target_path: SharedString,
    plan: RemoveWorktreePlan,
    branch: Option<String>,
    delete_branch_too: bool,
    delete_folder: bool,
    stage: Stage,
    error: Option<SharedString>,
    allow_force: bool,
    submitting: bool,
    workspace: WeakEntity<Workspace>,
}

impl RemoveWorktreeModal {
    pub fn new(
        workspace: WeakEntity<Workspace>,
        target: LaneRef,
        target_label: impl Into<SharedString>,
        target_path: impl Into<SharedString>,
        plan: RemoveWorktreePlan,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::inspect(&plan, cx);
        Self {
            focus_handle: cx.focus_handle(),
            target,
            target_label: target_label.into(),
            target_path: target_path.into(),
            plan,
            branch: None,
            delete_branch_too: false,
            delete_folder: false,
            stage: Stage::Inspecting,
            error: None,
            allow_force: false,
            submitting: false,
            workspace,
        }
    }

    fn inspect(plan: &RemoveWorktreePlan, cx: &mut Context<Self>) {
        let repo = crate::git_host::GitHost::global(cx).open(&plan.repo_root);
        let path = plan.path.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    repo.lane_removal_state(&path)
                        .map_err(s::modal::remove_lane_check_failed)
                })
                .await;
            // SILENT-OK: a closed modal has no inspection result to display.
            let _ = this.update(cx, |modal, cx| {
                match result {
                    Ok(state) => modal.stage = Stage::Ready(state),
                    Err(error) => {
                        modal.stage = Stage::InspectionFailed;
                        modal.error = Some(error.into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn with_branch(mut self, branch: Option<String>) -> Self {
        self.branch = branch;
        self
    }

    fn toggle_delete_branch(&mut self, cx: &mut Context<Self>) {
        if self.branch.is_some() && !self.submitting {
            self.delete_branch_too = !self.delete_branch_too;
            cx.notify();
        }
    }

    fn toggle_delete_folder(&mut self, cx: &mut Context<Self>) {
        if self.stage == Stage::Ready(LaneRemovalState::Unregistered) && !self.submitting {
            self.delete_folder = !self.delete_folder;
            if !self.delete_folder {
                self.delete_branch_too = false;
            }
            cx.notify();
        }
    }

    fn dismiss(&mut self, window: &mut Window, cx: &mut App) {
        if !self.submitting {
            window.close_dialog(cx);
        }
    }

    #[cfg(test)]
    fn allow_force(&self) -> bool {
        self.allow_force
    }
    #[cfg(test)]
    fn target(&self) -> LaneRef {
        self.target
    }
    #[cfg(test)]
    fn delete_branch_too(&self) -> bool {
        self.delete_branch_too
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.submitting {
            return;
        }
        let action = match self.stage {
            Stage::Inspecting => return,
            Stage::InspectionFailed => {
                self.stage = Stage::Inspecting;
                self.error = None;
                Self::inspect(&self.plan, cx);
                cx.notify();
                return;
            }
            Stage::Branch => {
                let Some(branch) = self.branch.clone() else {
                    return;
                };
                operation::Action::Branch(branch)
            }
            Stage::Ready(expected) => operation::Action::Worktree {
                expected,
                delete_folder: self.delete_folder,
                force: self.allow_force,
                branch: self
                    .delete_branch_too
                    .then(|| self.branch.clone())
                    .flatten(),
            },
        };
        self.submitting = true;
        self.error = None;
        cx.notify();
        let me = cx.entity().downgrade();
        let workspace = self.workspace.clone();
        let repo = crate::git_host::GitHost::global(cx).open(&self.plan.repo_root);
        let path = self.plan.path.clone();
        let lock_dir = self.plan.lock_dir.clone();
        let target = self.target;
        let diagnostic = format!(
            "repo={} target={}",
            self.plan.repo_root.display(),
            self.plan.path.display()
        );
        window
            .spawn(cx, async move |async_cx| {
                let result = async_cx
                    .background_executor()
                    .spawn(async move {
                        let result = operation::run(repo.as_ref(), &path, action);
                        if matches!(&result, Ok(operation::Outcome::Removed { .. })) {
                            crate::workspace::flow_paths::forget_lane_lock(lock_dir);
                        }
                        result
                    })
                    .await;
                // SILENT-OK: a closed workspace window needs no further UI cleanup.
                let _ = async_cx.update(|window, cx| {
                    // Disk success belongs to the workspace even if the dialog
                    // was closed while git was running or branch deletion failed.
                    if let Some(ws) = workspace.upgrade() {
                        ws.update(cx, |ws, cx| {
                            if matches!(&result, Ok(operation::Outcome::Removed { .. })) {
                                ws.finalize_remove_lane(target, window, cx);
                            }
                            let error = match &result {
                                Err(error) => Some(error),
                                Ok(operation::Outcome::Removed { branch_error }) => {
                                    branch_error.as_ref()
                                }
                                _ => None,
                            };
                            if let Some(error) = error {
                                let report =
                                    daruda_store::observability::error_report::ErrorReport::new(
                                        s::error::lane_remove_failed(),
                                    )
                                    .message(format!("{diagnostic}: {error}"))
                                    .at(file!(), line!())
                                    .dedup("lane.remove")
                                    .build();
                                ws.report_error(report, cx);
                            }
                        });
                    }
                    if let Some(me) = me.upgrade() {
                        me.update(cx, |modal, cx| modal.apply_result(result, window, cx));
                    }
                });
            })
            .detach();
    }

    fn apply_result(
        &mut self,
        result: Result<operation::Outcome, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.submitting = false;
        match result {
            Ok(operation::Outcome::Removed {
                branch_error: Some(error),
            }) => {
                self.stage = Stage::Branch;
                self.allow_force = false;
                self.error = Some(error.into());
            }
            Ok(
                operation::Outcome::Removed { branch_error: None }
                | operation::Outcome::BranchDeleted,
            ) => self.dismiss(window, cx),
            Ok(operation::Outcome::StateChanged(state)) => {
                self.stage = Stage::Ready(state);
                self.delete_folder = false;
                self.delete_branch_too = false;
                self.allow_force = false;
                self.error = Some(s::modal::remove_lane_state_changed().into());
            }
            Err(error) => {
                if self.stage == Stage::Ready(LaneRemovalState::Registered)
                    && (error.contains("modifications") || error.contains("--force"))
                {
                    self.allow_force = true;
                }
                self.error = Some(error.into());
            }
        }
        cx.notify();
    }

    fn handle_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev.keystroke.key.as_str() {
            "escape" => {
                self.dismiss(window, cx);
                cx.stop_propagation();
            }
            "enter" => {
                self.submit(window, cx);
                cx.stop_propagation();
            }
            _ => {}
        }
    }
}

impl Focusable for RemoveWorktreeModal {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ModalView for RemoveWorktreeModal {}

#[cfg(test)]
mod tests;
