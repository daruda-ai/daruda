//! An agent an approval card pinned is removed after the early check but
//! before its chat opens: the open is refused rather than handed to another
//! agent, no prompt reaches anything, and a worktree git already made is
//! kept and named in the refusal.

use super::*;
use crate::control::result::ControlError;
use crate::workspace::right_dock::task_start::TaskStartError;
use crate::workspace::{ChatAgent, ChatOpen};
use daruda_config::{AgentDefinition, AgentEntry, ModeChoice};
use daruda_store::tasks::{Lifecycle, TaskRunIn};

const OTHER: &str = "other";

fn claude() -> AgentDefinition {
    AgentDefinition::claude_default()
}

/// Claude, plus the agent the tests pin, which a reload then removes.
fn with_other() -> daruda_config::Config {
    let other = AgentDefinition {
        id: OTHER.into(),
        name: "Other Agent".into(),
        ..claude()
    };
    daruda_config::Config {
        agents: vec![AgentEntry::custom(claude()), AgentEntry::custom(other)],
        ..daruda_config::Config::default()
    }
}

/// The same catalog without the pinned agent, applied through the path a
/// Settings edit or a config-file change takes.
fn without_other() -> daruda_config::Config {
    daruda_config::Config {
        agents: vec![AgentEntry::custom(claude())],
        ..daruda_config::Config::default()
    }
}

fn pinned() -> ChatOpen {
    ChatOpen {
        agent: ChatAgent::Pinned(OTHER.into()),
        mode: ModeChoice::FollowDelegated,
    }
}

fn lane_count(ws: &Workspace) -> usize {
    ws.projects.iter().map(|p| p.lanes.len()).sum()
}

/// `worktree_create`'s agent goes while git runs: the worktree is kept as a
/// lane with nothing open in it, no other agent opens there, and the caller
/// is told which worktree it kept.
#[gpui::test]
async fn a_worktree_whose_pinned_agent_left_during_git_opens_no_chat(cx: &mut TestAppContext) {
    let fx = GitFixture::open(cx, &with_other(), |repo| repo);
    let (window, workspace, fake) = (fx.window, &fx.ws, &fx.fake);
    let _connections = cx.update(Workspace::fake_agent_connections_for_test);
    let (panes, lanes) = workspace.read_with(cx, |ws, _| (ws.all_pane_ids(), lane_count(ws)));
    let (answer, checkout) = cx
        .update_window(window.into(), |_, window, cx| {
            workspace.update(cx, |ws, cx| {
                let project = ws.projects[0].id;
                let checkout = ws
                    .control_lane_plan(project, "pinned", None)
                    .unwrap()
                    .new_path;
                fake_lane(fake, &checkout, "pinned");
                // The agent is there when the create is asked for, so git
                // runs; it is gone by the time git is done.
                let answer =
                    ws.control_create_lane(project, "pinned".into(), None, pinned(), window, cx);
                ws.reload_config(&without_other(), cx);
                (answer, checkout)
            })
        })
        .unwrap();
    cx.executor().allow_parking();
    let refused = answer.recv().await.expect("answered");
    cx.run_until_parked();
    let added = crate::test_support::added_lane(fake);
    assert_eq!(added.new_path, checkout, "git made the worktree");
    assert_eq!(added.new_branch.as_deref(), Some("pinned"));
    workspace.read_with(cx, |ws, _| {
        assert_eq!(lane_count(ws), lanes + 1, "and it is kept as a lane");
        let lane = ws.lane_ref_at(&checkout).expect("registered");
        assert_eq!(
            refused,
            Err(ControlError::AgentUnavailableWorktreeKept {
                worktree: crate::control::result::LaneHandle::new(ws.uuid(), lane)
            }),
            "the refusal names the worktree it kept"
        );
        assert!(
            ws.main_area
                .runtimes
                .get(&lane)
                .is_none_or(|rt| rt.panes.is_empty()),
            "with nothing open in it"
        );
        assert_eq!(ws.all_pane_ids(), panes, "no other agent opened anywhere");
    });
}

/// A task start's pinned agent goes while the start is recorded: the start
/// fails as a missing agent, and no pane opens to take its prompt.
#[gpui::test]
fn an_existing_lane_task_whose_pinned_agent_left_opens_no_chat(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let project = daruda_store::project::Project::from_path(root.path());
    let (window, workspace) = build_workspace_with(cx, &with_other(), Some(project));
    cx.run_until_parked();
    let _connections = cx.update(Workspace::fake_agent_connections_for_test);
    let (id, answer, panes) = cx
        .update_window(window.into(), |_, window, cx| {
            workspace.update(cx, |ws, cx| {
                let path = ws.active_lane().unwrap().path.clone();
                let mut spec = crate::runs::seed::spec(ws.projects[0].uuid, "Fix it");
                spec.run_in = TaskRunIn::ExistingLane { path };
                let id = crate::runs::seed::task(spec, None, cx);
                let panes = ws.all_pane_ids();
                let submitter = crate::runs::FlowSubmitter::Orchestrator { solicited: false };
                let answer = ws.begin_task_start_for(&id, submitter, pinned(), window, cx);
                ws.reload_config(&without_other(), cx);
                (id, answer, panes)
            })
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(answer.try_recv(), Ok(Err(TaskStartError::AgentUnavailable)));
    workspace.read_with(cx, |ws, _| assert_eq!(ws.all_pane_ids(), panes));
    let lifecycle = cx.update(|cx| {
        cx.global::<crate::runs::GlobalTasks>()
            .get(&id)
            .unwrap()
            .lifecycle
            .clone()
    });
    assert!(
        matches!(
            lifecycle,
            Lifecycle::Error {
                cause: daruda_store::tasks::machine::ErrorCause::StartFailed(
                    daruda_store::tasks::machine::StartFailure::AgentMissing
                ),
                ..
            }
        ),
        "the recorded start fails as a missing agent: {lifecycle:?}"
    );
}

/// A new-worktree task start's pinned agent goes while the start is
/// recorded: git still makes the worktree, which is kept as a lane with
/// nothing open; the start fails as a missing agent, and the control answer
/// names the worktree it left.
#[gpui::test]
async fn a_new_worktree_task_whose_pinned_agent_left_keeps_its_worktree(cx: &mut TestAppContext) {
    let fx = GitFixture::open(cx, &with_other(), |repo| repo);
    let (window, workspace, fake, root) = (fx.window, &fx.ws, &fx.fake, &fx.root);
    let _connections = cx.update(Workspace::fake_agent_connections_for_test);
    let (id, answer, panes, lanes) = cx
        .update_window(window.into(), |_, window, cx| {
            workspace.update(cx, |ws, cx| {
                let mut spec = crate::runs::seed::spec(ws.projects[0].uuid, "Fix it");
                spec.run_in = TaskRunIn::NewWorktree;
                spec.branch_name = "pinned-task".into();
                let checkout =
                    crate::workspace::lane_ops::lane_checkout_path(root, &spec.branch_name);
                fake_lane(fake, &checkout, &spec.branch_name);
                let id = crate::runs::seed::task(spec, None, cx);
                let panes = ws.all_pane_ids();
                let lanes = lane_count(ws);
                let submitter = crate::runs::FlowSubmitter::Orchestrator { solicited: false };
                let answer = ws.control_task_start(&id, submitter, pinned(), window, cx);
                ws.reload_config(&without_other(), cx);
                (id, answer, panes, lanes)
            })
        })
        .unwrap();
    cx.executor().allow_parking();
    let refused = answer.recv().await.expect("answered");
    cx.run_until_parked();
    workspace.read_with(cx, |ws, _| {
        assert_eq!(
            lane_count(ws),
            lanes + 1,
            "git made the worktree, kept as a lane"
        );
        let Err(ControlError::AgentUnavailableWorktreeKept { worktree }) = refused else {
            panic!("the answer names the worktree the start left: {refused:?}");
        };
        assert_eq!(worktree.workspace, ws.uuid());
        let lane = worktree.lane_ref();
        let kept = ws.lane_for(lane).expect("the task's worktree is registered");
        let added = crate::test_support::added_lane(fake);
        assert_eq!(added.new_path, kept.path, "git made it");
        assert_eq!(added.new_branch.as_deref(), Some("pinned-task"));
        assert!(
            matches!(&kept.kind, daruda_store::project::LaneKind::Git { branch: Some(b), .. } if b == "pinned-task"),
            "on the task's branch: {:?}",
            kept.kind
        );
        assert!(
            ws.main_area
                .runtimes
                .get(&lane)
                .is_none_or(|rt| rt.panes.is_empty()),
            "with nothing open in it"
        );
        assert_eq!(ws.all_pane_ids(), panes, "no other agent opened anywhere");
    });
    let lifecycle = cx.update(|cx| {
        cx.global::<crate::runs::GlobalTasks>()
            .get(&id)
            .unwrap()
            .lifecycle
            .clone()
    });
    assert!(
        matches!(
            lifecycle,
            Lifecycle::Error {
                cause: daruda_store::tasks::machine::ErrorCause::StartFailed(
                    daruda_store::tasks::machine::StartFailure::AgentMissing
                ),
                ..
            }
        ),
        "the recorded start fails as a missing agent: {lifecycle:?}"
    );
}
