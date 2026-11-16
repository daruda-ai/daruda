//! Creating a worktree on behalf of the control surface.
//!
//! Its own file because it is the one control operation that cannot answer in
//! one turn of the event loop: `git worktree add` is blocking, so this is
//! plan → background git → register, the same three steps the task workflow
//! runs. `finalize_chat_lane` owns the third one, so nothing here becomes a
//! second way to make a lane.
//!
//! The orchestration lives inside `crate::workspace` so `CreateWorktreePlan`
//! does not have to leave it; the dispatcher only sees a channel.

use gpui::{Context, Window};

use daruda_store::observability::error_report::{ErrorReport, ErrorSeverity};
use daruda_store::project::{LaneRef, ProjectId};

use crate::control::result::ControlError;
use crate::remote_channel::bridge::PaneRef;
use crate::workspace::Workspace;
use crate::workspace::lane_ops::{WorktreeCreate, add_new_branch};

/// What a finished creation reports: the worktree, and the chat that came with
/// it, so a caller can talk to the new agent without listing again.
pub(crate) type CreatedLane = Result<(LaneRef, PaneRef), ControlError>;

/// Why a worktree git made was never registered as a lane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::workspace) enum Unregistered {
    ProjectClosed,
    WindowClosed,
}

impl Unregistered {
    /// The answer for a checkout left at `path`, with the reason in the fixed
    /// English a control answer carries.
    pub(in crate::workspace) fn at(self, path: std::path::PathBuf) -> ControlError {
        let detail = match self {
            Self::ProjectClosed => "its project closed before daruda registered it",
            Self::WindowClosed => "its window closed before daruda registered it",
        };
        ControlError::WorktreeUnregistered {
            path,
            detail: detail.to_owned(),
        }
    }
}

impl Workspace {
    /// Create a worktree in `project` and open `chat` in it.
    ///
    /// Answers on the returned channel once the git work and the registration
    /// are both done. A caller that stops listening cannot stall it — the
    /// channel is bounded at one and the send is never awaited.
    pub(crate) fn control_create_lane(
        &mut self,
        project: ProjectId,
        name: String,
        base_ref: Option<String>,
        chat: crate::workspace::ChatOpen,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> smol::channel::Receiver<CreatedLane> {
        let (tx, rx) = smol::channel::bounded(1);
        // Before git, so a card whose agent is already gone creates nothing;
        // finalize checks again for one removed while git runs.
        if self.resolve_chat_agent(&chat.agent).is_err() {
            let _ = tx.try_send(Err(ControlError::AgentUnavailable));
            return rx;
        }
        let plan = match self.control_lane_plan(project, &name, base_ref) {
            Ok(plan) => plan,
            Err(e) => {
                let _ = tx.try_send(Err(e));
                return rx;
            }
        };
        // Two `git worktree add` runs against one repo can leave a
        // half-created checkout, and an agent asking while another create is
        // in flight is exactly that case.
        let Some(lock) = self.lock_repo(&plan.repo_root) else {
            let _ = tx.try_send(Err(ControlError::LaneCreateBusy));
            return rx;
        };
        let create = WorktreeCreate {
            lock,
            new_path: plan.new_path.clone(),
            dedup: "control.lane_create.unregistered",
        };
        let created = self.spawn_worktree_create(
            create,
            add_new_branch(&plan),
            move |ws, added, window, cx| match added {
                Err(detail) => {
                    ws.report_error(
                        ErrorReport::new(crate::surface::strings::error::lane_create_failed())
                            .severity(ErrorSeverity::Error)
                            .at(file!(), line!())
                            .with_context("detail", detail.clone())
                            .dedup("control.lane_create")
                            .build(),
                        cx,
                    );
                    // git's message rides out with the code: the caller is a
                    // model that has to fix an argument, and the log it
                    // cannot read is no help to it.
                    Err(ControlError::LaneCreateFailed { detail })
                }
                Ok(()) => ws.control_lane_finalize(plan, project, chat, window, cx),
            },
            window,
            cx,
        );
        cx.spawn(async move |_, _| {
            // Once git made the checkout, a window gone before it is
            // registered leaves it on disk with no lane: name its path.
            let outcome = created.await.unwrap_or_else(|gone| {
                Err(match gone.left_at {
                    Some(path) => Unregistered::WindowClosed.at(path),
                    None => ControlError::TargetGone,
                })
            });
            let _ = tx.try_send(outcome);
        })
        .detach();
        rx
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext as _, TestAppContext};

    /// A project that is not open cannot be planned against, and the caller
    /// hears so immediately rather than after a git attempt.
    #[gpui::test]
    async fn creating_in_a_missing_project_refuses_before_touching_git(cx: &mut TestAppContext) {
        let fixture = crate::test_support::workspace_with_agent_chat(cx);
        let rx = cx
            .update_window(fixture.window.into(), |_, window, cx| {
                fixture.workspace.update(cx, |ws, cx| {
                    let chat = crate::workspace::ChatOpen::orchestrator_for_test();
                    ws.control_create_lane(9_999, "x".into(), None, chat, window, cx)
                })
            })
            .expect("window is live");
        assert_eq!(
            rx.recv().await,
            Ok(Err(ControlError::TargetGone)),
            "answered without spawning anything"
        );
    }
}
