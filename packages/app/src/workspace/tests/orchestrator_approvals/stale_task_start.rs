//! A task start's card names where the task runs. An edit that moves it
//! elsewhere before the start reads the task refuses the approval as stale:
//! no worktree is made, no pane opens, and the orchestrator's worktree budget
//! is neither spent nor held.

use super::disclosure::{ask, host};
use super::*;
use crate::control::{guards, result::ControlError, spec::GatedCommand};
use crate::runs::{GlobalTasks, RunsHost, TaskIntent};
use crate::workspace::tests::{GitFixture, fake_lane};
use crate::workspace::{ShownTaskStart, Workspace};
use daruda_store::tasks::machine::TaskEdit;
use daruda_store::tasks::{Lifecycle, TaskRunIn};
use gpui::App;

/// `task` moved to run `run_in`, everything else as it is: the edit a
/// desktop Backlog change sends.
fn moved(task: &str, run_in: TaskRunIn, cx: &App) -> TaskIntent {
    let spec = cx.global::<GlobalTasks>().get(task).unwrap().spec.clone();
    TaskIntent::Edit {
        id: task.into(),
        edit: TaskEdit {
            title: spec.title,
            prompt: spec.prompt,
            notes: spec.notes,
            base_worktree_path: spec.base_worktree_path,
            branch: None,
            run_in: Some(run_in),
            project: None,
            flow: spec.flow,
        },
    }
}

fn lane_count(ws: &Workspace) -> usize {
    ws.projects.iter().map(|p| p.lanes.len()).sum()
}

fn lifecycle(task: &str, cx: &mut TestAppContext) -> Lifecycle {
    cx.update(|cx| {
        cx.global::<GlobalTasks>()
            .get(task)
            .unwrap()
            .lifecycle
            .clone()
    })
}

/// Edited between the card and the approval, in each direction: a new
/// worktree moved into an existing one would spend a slot for nothing, and
/// an existing one moved to a new worktree would make one with no slot.
#[gpui::test]
async fn a_task_start_moved_elsewhere_while_its_card_waits_is_refused(cx: &mut TestAppContext) {
    let (fixture, lane, _claude) = host(cx);
    let _connections = cx.update(Workspace::fake_agent_connections_for_test);
    let here = fixture.workspace.read_with(cx, |ws, _| {
        ws.lane_for(lane.lane_ref()).unwrap().path.clone()
    });
    // One slot left, so a slot held by the refused start would show.
    cx.update(|cx| {
        for _ in 0..guards::AGENT_LANE_BUDGET - 1 {
            guards::note_agent_created_lane(cx);
        }
    });
    cx.executor().allow_parking();
    let moves = [
        (
            TaskRunIn::NewWorktree,
            TaskRunIn::ExistingLane { path: here.clone() },
        ),
        (
            TaskRunIn::ExistingLane { path: here },
            TaskRunIn::NewWorktree,
        ),
    ];
    for (from, to) in moves {
        let task = fixture.workspace.update(cx, |ws, cx| {
            let mut spec = crate::runs::seed::spec(ws.projects[0].uuid, "Login");
            spec.run_in = from.clone();
            crate::runs::seed::task(spec, None, cx)
        });
        let (id, outcome, _card) = ask(GatedCommand::TaskStart { task: task.clone() }, cx);
        let before = fixture
            .workspace
            .read_with(cx, |ws, _| (ws.all_pane_ids(), lane_count(ws)));
        let edited = cx.update(|cx| RunsHost::task(moved(&task, to.clone(), cx), cx));
        edited.await.unwrap();
        cx.update(|cx| assert!(resolve_desktop(id.0, "approve", cx)));
        let refused = outcome.recv().await.unwrap().unwrap();
        cx.run_until_parked();
        assert_eq!(
            refused,
            Err(ControlError::ApprovalStale),
            "{from:?} -> {to:?}"
        );
        let after = fixture
            .workspace
            .read_with(cx, |ws, _| (ws.all_pane_ids(), lane_count(ws)));
        assert_eq!(after, before, "nothing opened or was made");
        assert_eq!(lifecycle(&task, cx), Lifecycle::Backlog);
        cx.update(|cx| {
            assert_eq!(
                guards::spent_budget_for_test(cx),
                guards::AGENT_LANE_BUDGET - 1,
                "no slot was spent"
            );
            assert_eq!(guards::guard_agent_budget(cx), Ok(()), "nor held");
        });
    }
}

/// An edit still being committed when an approved start reads its task:
/// the start waits for it, finds a location its card never named, and
/// refuses before the repository lock, git or the log. Without the check, a
/// task moved to a new worktree would reach git here with a worktree to make.
#[gpui::test]
async fn an_edit_landing_as_an_approved_start_reads_its_task_refuses_it(cx: &mut TestAppContext) {
    let fx = GitFixture::open(cx, &daruda_config::Config::default(), |repo| repo);
    let _connections = cx.update(Workspace::fake_agent_connections_for_test);
    let here = fx
        .ws
        .read_with(cx, |ws, _| ws.active_lane().unwrap().path.clone());
    cx.executor().allow_parking();
    let moves = [
        (
            "to-existing",
            TaskRunIn::NewWorktree,
            TaskRunIn::ExistingLane { path: here.clone() },
        ),
        (
            "to-new",
            TaskRunIn::ExistingLane { path: here },
            TaskRunIn::NewWorktree,
        ),
    ];
    for (branch, from, to) in moves {
        let task = fx.ws.update(cx, |ws, cx| {
            let mut spec = crate::runs::seed::spec(ws.projects[0].uuid, "Fix it");
            spec.run_in = from;
            spec.branch_name = branch.into();
            let checkout = crate::workspace::lane_ops::lane_checkout_path(&fx.root, branch);
            fake_lane(&fx.fake, &checkout, branch);
            crate::runs::seed::task(spec, None, cx)
        });
        let shown =
            cx.update(|cx| ShownTaskStart::of(cx.global::<GlobalTasks>().get(&task).unwrap()));
        let (before, edited, answer) = cx
            .update_window(fx.window.into(), |_, window, cx| {
                fx.ws.update(cx, |ws, cx| {
                    let before = (ws.all_pane_ids(), lane_count(ws));
                    let edited = RunsHost::task(moved(&task, to, cx), cx);
                    assert!(!RunsHost::settled(cx), "the edit is still landing");
                    let submitter = crate::runs::FlowSubmitter::Orchestrator { solicited: false };
                    let agent = crate::workspace::ChatAgent::Preferred(None);
                    let answer =
                        ws.control_task_start(&task, submitter, agent, Some(shown), window, cx);
                    (before, edited, answer)
                })
            })
            .unwrap();
        edited.await.unwrap();
        let refused = answer.recv().await.expect("answered");
        cx.run_until_parked();
        assert_eq!(refused, Err(ControlError::ApprovalStale), "{branch}");
        let added = fx
            .fake
            .calls()
            .into_iter()
            .any(|call| matches!(call, crate::lane::git::GitCall::AddLane { .. }));
        assert!(!added, "git made no worktree");
        let after = fx
            .ws
            .read_with(cx, |ws, _| (ws.all_pane_ids(), lane_count(ws)));
        assert_eq!(after, before, "nothing opened or was made");
        assert_eq!(lifecycle(&task, cx), Lifecycle::Backlog);
    }
}
