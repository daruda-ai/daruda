//! A start or a creation that fails after git made its worktree names where
//! that worktree stays: the lane it was registered as, or its path when its
//! project or window closed before any lane held it.

use super::*;
use crate::control::result::{ControlError, LaneHandle};
use crate::workspace::ChatOpen;
use crate::workspace::control_lane_ops::Unregistered;
use daruda_store::tasks::machine::{ErrorCause, StartFailure};
use daruda_store::tasks::{Lifecycle, TaskRunIn};
use gpui::Context;
use std::path::PathBuf;

const BRANCH: &str = "kept-task";

/// A Backlog task on `BRANCH` in a worktree of its own, and where git will
/// make that worktree.
fn new_worktree_task(
    fx: &GitFixture,
    ws: &Workspace,
    cx: &mut Context<Workspace>,
) -> (String, PathBuf) {
    let mut spec = crate::runs::seed::spec(ws.projects[0].uuid, "Fix it");
    spec.run_in = TaskRunIn::NewWorktree;
    spec.branch_name = BRANCH.into();
    let checkout = crate::workspace::lane_ops::lane_checkout_path(&fx.root, BRANCH);
    fake_lane(&fx.fake, &checkout, BRANCH);
    (crate::runs::seed::task(spec, None, cx), checkout)
}

/// Start `id` as the orchestrator does, then run `meanwhile` before git
/// returns.
fn start_then(
    fx: &GitFixture,
    id: &str,
    cx: &mut TestAppContext,
    meanwhile: impl FnOnce(&mut Workspace, &mut gpui::Window, &mut Context<Workspace>),
) -> smol::channel::Receiver<crate::workspace::ControlTaskStart> {
    cx.update_window(fx.window.into(), |_, window, cx| {
        fx.ws.update(cx, |ws, cx| {
            let submitter = crate::runs::FlowSubmitter::Orchestrator { solicited: false };
            let agent = crate::workspace::ChatAgent::Preferred(None);
            let answer = ws.control_task_start(id, submitter, agent, None, window, cx);
            meanwhile(ws, window, cx);
            answer
        })
    })
    .expect("window is live")
}

fn lifecycle(id: &str, cx: &mut TestAppContext) -> Lifecycle {
    cx.update(|cx| {
        cx.global::<crate::runs::GlobalTasks>()
            .get(id)
            .expect("the task")
            .lifecycle
            .clone()
    })
}

fn failed_with(lifecycle: &Lifecycle, failure: StartFailure) -> bool {
    matches!(
        lifecycle,
        Lifecycle::Error { cause: ErrorCause::StartFailed(f), .. } if *f == failure
    )
}

/// The project closes while git makes the task's worktree: no lane can hold
/// it, so the answer names its path, and the start fails as a closed project.
#[gpui::test]
async fn a_task_whose_project_closed_during_git_names_the_worktree_left(cx: &mut TestAppContext) {
    let fx = GitFixture::open(cx, &daruda_config::Config::default(), |repo| repo);
    let _connections = cx.update(Workspace::fake_agent_connections_for_test);
    let (id, checkout) = fx.ws.update(cx, |ws, cx| new_worktree_task(&fx, ws, cx));
    let answer = start_then(&fx, &id, cx, |ws, window, cx| {
        ws.close_active_project(window, cx)
    });
    cx.executor().allow_parking();
    let refused = answer.recv().await.expect("answered");
    cx.run_until_parked();
    assert_eq!(crate::test_support::added_lane(&fx.fake).new_path, checkout);
    assert_eq!(
        refused,
        Err(Unregistered::ProjectClosed.at(checkout.clone())),
        "the answer names the checkout git left"
    );
    fx.ws
        .read_with(cx, |ws, _| assert_eq!(ws.lane_ref_at(&checkout), None));
    let lifecycle = lifecycle(&id, cx);
    assert!(
        failed_with(&lifecycle, StartFailure::ProjectNotOpen),
        "{lifecycle:?}"
    );
}

/// `worktree_create`'s project closes while git runs: the answer names the
/// checkout's path rather than a worktree that was never listed.
#[gpui::test]
async fn a_worktree_whose_project_closed_during_git_names_its_path(cx: &mut TestAppContext) {
    let fx = GitFixture::open(cx, &daruda_config::Config::default(), |repo| repo);
    let _connections = cx.update(Workspace::fake_agent_connections_for_test);
    let (answer, checkout) = cx
        .update_window(fx.window.into(), |_, window, cx| {
            fx.ws.update(cx, |ws, cx| {
                let project = ws.projects[0].id;
                let checkout = ws
                    .control_lane_plan(project, "orphan", None)
                    .unwrap()
                    .new_path;
                fake_lane(&fx.fake, &checkout, "orphan");
                let chat = ChatOpen::orchestrator_for_test();
                let answer =
                    ws.control_create_lane(project, "orphan".into(), None, chat, window, cx);
                ws.close_active_project(window, cx);
                (answer, checkout)
            })
        })
        .unwrap();
    cx.executor().allow_parking();
    let refused = answer.recv().await.expect("answered");
    cx.run_until_parked();
    assert_eq!(crate::test_support::added_lane(&fx.fake).new_path, checkout);
    assert_eq!(
        refused,
        Err(Unregistered::ProjectClosed.at(checkout.clone()))
    );
    fx.ws
        .read_with(cx, |ws, _| assert_eq!(ws.lane_ref_at(&checkout), None));
}

/// The window closes while git makes `worktree_create`'s checkout: nothing
/// is left to register it in, and the answer names its path.
#[gpui::test]
async fn a_worktree_whose_window_closed_during_git_names_its_path(cx: &mut TestAppContext) {
    let fx = GitFixture::open(cx, &daruda_config::Config::default(), |repo| repo);
    let _connections = cx.update(Workspace::fake_agent_connections_for_test);
    let (answer, checkout) = cx
        .update_window(fx.window.into(), |_, window, cx| {
            fx.ws.update(cx, |ws, cx| {
                let project = ws.projects[0].id;
                let checkout = ws
                    .control_lane_plan(project, "orphan", None)
                    .unwrap()
                    .new_path;
                fake_lane(&fx.fake, &checkout, "orphan");
                let chat = ChatOpen::orchestrator_for_test();
                let answer =
                    ws.control_create_lane(project, "orphan".into(), None, chat, window, cx);
                window.remove_window();
                (answer, checkout)
            })
        })
        .unwrap();
    cx.executor().allow_parking();
    let refused = answer.recv().await.expect("answered");
    cx.run_until_parked();
    assert_eq!(crate::test_support::added_lane(&fx.fake).new_path, checkout);
    assert_eq!(refused, Err(Unregistered::WindowClosed.at(checkout)));
}

/// The new chat's session fails as it opens, so the prompt never reaches it:
/// the worktree and its pane stay, and the answer names that worktree.
#[gpui::test]
async fn a_prompt_the_new_chat_never_took_names_the_worktree_kept(cx: &mut TestAppContext) {
    let fx = GitFixture::open(cx, &daruda_config::Config::default(), |repo| repo);
    let _connections = cx.update(Workspace::fake_agent_connections_for_test);
    let before = fx.ws.read_with(cx, |ws, _| ws.all_pane_ids());
    // Every pane the start opens is failed before its prompt is delivered.
    let _failing = cx.update(|cx| {
        cx.observe(&fx.ws, move |ws, cx| {
            let opened: Vec<_> = ws
                .read(cx)
                .all_pane_ids()
                .into_iter()
                .filter(|pane| !before.contains(pane))
                .filter_map(|pane| ws.read(cx).agent_chat_view(pane).cloned())
                .collect();
            for view in opened {
                view.update(cx, |view, _| {
                    view.set_status_for_test(daruda_chat::view::AgentSessionStatus::Error {
                        message: "the adapter exited".into(),
                        remedy: daruda_acp::Remedy::NoneAvailable,
                    })
                });
            }
        })
    });
    let (id, checkout) = fx.ws.update(cx, |ws, cx| new_worktree_task(&fx, ws, cx));
    let answer = start_then(&fx, &id, cx, |_, _, _| {});
    cx.executor().allow_parking();
    let refused = answer.recv().await.expect("answered");
    cx.run_until_parked();
    fx.ws.read_with(cx, |ws, _| {
        let lane = ws
            .lane_ref_at(&checkout)
            .expect("the worktree is registered");
        assert!(
            ws.main_area
                .runtimes
                .get(&lane)
                .is_some_and(|rt| !rt.panes.is_empty()),
            "with the chat that opened in it"
        );
        assert_eq!(
            refused,
            Err(ControlError::TaskStartFailedWorktreeKept {
                worktree: LaneHandle::new(ws.uuid(), lane),
                // Fixed English, whatever the person's language.
                detail: "the prompt could not reach the pane, which closed".into(),
            })
        );
    });
    let lifecycle = lifecycle(&id, cx);
    assert!(
        !matches!(lifecycle, Lifecycle::Running { .. }),
        "{lifecycle:?}"
    );
}

/// The task is cancelled once its start is recorded, before git returns:
/// the worktree is still registered, no prompt is sent, and the answer
/// names the worktree.
#[gpui::test]
async fn a_cancel_landing_while_the_worktree_is_made_names_the_worktree_kept(
    cx: &mut TestAppContext,
) {
    let fx = GitFixture::open(cx, &daruda_config::Config::default(), |repo| repo);
    let probes = cx.update(Workspace::fake_agent_connections_for_test);
    // The start is recorded on the spot, so the cancel lands after it and
    // before git's answer is read.
    cx.update(crate::runs::RunsHost::inline_commits_for_test);
    let (id, checkout) = fx.ws.update(cx, |ws, cx| new_worktree_task(&fx, ws, cx));
    let answer = start_then(&fx, &id, cx, |ws, _, cx| {
        let exec = cx
            .global::<crate::runs::GlobalTasks>()
            .get(&id)
            .and_then(|task| task.current_exec().cloned())
            .expect("the start is recorded");
        ws.cancel_execution(exec, cx).detach();
    });
    cx.executor().allow_parking();
    let refused = answer.recv().await.expect("answered");
    cx.run_until_parked();
    fx.ws.read_with(cx, |ws, _| {
        let lane = ws
            .lane_ref_at(&checkout)
            .expect("the worktree is registered");
        assert_eq!(
            refused,
            Err(ControlError::TaskStartFailedWorktreeKept {
                worktree: LaneHandle::new(ws.uuid(), lane),
                detail: ControlError::TaskNotBacklog.to_string(),
            })
        );
    });
    assert!(
        probes
            .borrow_mut()
            .iter_mut()
            .all(|probe| probe.drain().0 == 0),
        "no prompt was sent"
    );
    assert!(matches!(lifecycle(&id, cx), Lifecycle::Cancelled { .. }));
}
