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

/// `task`'s start as a card written now would describe it.
fn shown_now(fx: &GitFixture, task: &str, cx: &App) -> ShownTaskStart {
    fx.ws
        .read(cx)
        .shown_task_start(cx.global::<GlobalTasks>().get(task).unwrap())
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
        let shown = cx.update(|cx| shown_now(&fx, &task, cx));
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

/// `task` retitled and reworded, everything else as it is.
fn reworded(task: &str, title: &str, prompt: &str, cx: &App) -> TaskIntent {
    let spec = cx.global::<GlobalTasks>().get(task).unwrap().spec.clone();
    TaskIntent::Edit {
        id: task.into(),
        edit: TaskEdit {
            title: title.into(),
            prompt: prompt.into(),
            notes: spec.notes,
            base_worktree_path: spec.base_worktree_path,
            branch: None,
            run_in: None,
            project: None,
            flow: spec.flow,
        },
    }
}

/// A new worktree's chat task, `title` asking `prompt`, its checkout
/// scripted so git can make it.
fn new_worktree_task(fx: &GitFixture, branch: &str, prompt: &str, cx: &mut App) -> String {
    let project = fx.ws.read(cx).projects[0].uuid;
    let mut spec = crate::runs::seed::spec(project, "Original title");
    spec.prompt = prompt.into();
    spec.branch_name = branch.into();
    let checkout = crate::workspace::lane_ops::lane_checkout_path(&fx.root, branch);
    fake_lane(&fx.fake, &checkout, branch);
    crate::runs::seed::task(spec, None, cx)
}

/// Reworded once its start is recorded, while git makes the worktree: the
/// agent is still told what the start read, the card's words for an
/// approved start and the person's for a desktop one.
#[gpui::test]
async fn a_task_reworded_after_its_start_is_recorded_tells_its_agent_the_original(
    cx: &mut TestAppContext,
) {
    let fx = GitFixture::open(cx, &daruda_config::Config::default(), |repo| repo);
    let probes = cx.update(Workspace::fake_agent_connections_for_test);
    cx.executor().allow_parking();
    for (branch, approved) in [("approved", true), ("desktop", false)] {
        let task = cx.update(|cx| new_worktree_task(&fx, branch, "Original prompt", cx));
        let shown = approved.then(|| cx.update(|cx| shown_now(&fx, &task, cx)));
        let (edited, answer) = cx
            .update_window(fx.window.into(), |_, window, cx| {
                fx.ws.update(cx, |ws, cx| {
                    assert!(RunsHost::settled(cx), "the start reads the task at once");
                    let submitter = crate::runs::FlowSubmitter::Orchestrator { solicited: false };
                    let agent = crate::workspace::ChatAgent::Preferred(None);
                    let answer = ws.control_task_start(&task, submitter, agent, shown, window, cx);
                    let edited =
                        RunsHost::task(reworded(&task, "Edited title", "Edited prompt", cx), cx);
                    (edited, answer)
                })
            })
            .unwrap();
        edited
            .await
            .expect("a started task's words can still change");
        let started = answer.recv().await.expect("answered");
        cx.run_until_parked();
        assert!(started.is_ok(), "{branch}: {started:?}");
        let prompts: Vec<String> = probes
            .borrow_mut()
            .iter_mut()
            .flat_map(|probe| probe.drain_prompts())
            .collect();
        assert_eq!(prompts.len(), 1, "{branch}: {prompts:?}");
        assert!(
            prompts[0].contains("Original title") && prompts[0].contains("Original prompt"),
            "{branch}: {}",
            prompts[0]
        );
        assert!(!prompts[0].contains("Edited"), "{branch}: {}", prompts[0]);
    }
}

/// An edit proposed after an approved start began waiting for earlier
/// commits, and still landing when that wait ends: the start reads the task
/// only once nothing is landing, so it sees this edit too and refuses it,
/// rather than checking the earlier words and recording the start after the
/// edit, which would tell the agent words its card never showed.
#[gpui::test(iterations = 20)]
async fn an_edit_proposed_while_an_approved_start_waits_is_read_before_it_starts(
    cx: &mut TestAppContext,
) {
    let fx = GitFixture::open(cx, &daruda_config::Config::default(), |repo| repo);
    let probes = cx.update(Workspace::fake_agent_connections_for_test);
    cx.executor().allow_parking();
    let task = cx.update(|cx| new_worktree_task(&fx, "reworded", "Original prompt", cx));
    // The card was written once the first edit landed.
    let shown = cx.update(|cx| {
        let mut record = cx.global::<GlobalTasks>().get(&task).unwrap().clone();
        record.spec.prompt = "Shown prompt".into();
        fx.ws.read(cx).shown_task_start(&record)
    });
    let (first, second, answer) = cx
        .update_window(fx.window.into(), |_, window, cx| {
            fx.ws.update(cx, |ws, cx| {
                let first =
                    RunsHost::task(reworded(&task, "Original title", "Shown prompt", cx), cx);
                assert!(!RunsHost::settled(cx), "the first edit is still landing");
                let submitter = crate::runs::FlowSubmitter::Orchestrator { solicited: false };
                let agent = crate::workspace::ChatAgent::Preferred(None);
                let answer =
                    ws.control_task_start(&task, submitter, agent, Some(shown), window, cx);
                let second =
                    RunsHost::task(reworded(&task, "Original title", "Unseen prompt", cx), cx);
                (first, second, answer)
            })
        })
        .unwrap();
    first.await.unwrap();
    second.await.unwrap();
    let refused = answer.recv().await.expect("answered");
    cx.run_until_parked();
    assert_eq!(refused, Err(ControlError::ApprovalStale));
    let added = fx
        .fake
        .calls()
        .into_iter()
        .any(|call| matches!(call, crate::lane::git::GitCall::AddLane { .. }));
    assert!(!added, "git made no worktree");
    assert_eq!(lifecycle(&task, cx), Lifecycle::Backlog);
    let prompts: Vec<String> = probes
        .borrow_mut()
        .iter_mut()
        .flat_map(|probe| probe.drain_prompts())
        .collect();
    assert!(prompts.is_empty(), "{prompts:?}");
}

/// Ask to start `task`, let `change` land while its card waits, then
/// approve: the start is refused as stale, nothing opens or is made, the
/// task stays in Backlog, and no worktree slot is spent or held.
async fn refused_after_change(
    fixture: &crate::test_support::ControlFixture,
    task: &str,
    what: &str,
    change: impl AsyncFnOnce(&mut TestAppContext),
    cx: &mut TestAppContext,
) {
    let spent = cx.update(|cx| guards::spent_budget_for_test(cx));
    let (id, outcome, _card) = ask(GatedCommand::TaskStart { task: task.into() }, cx);
    let before = fixture
        .workspace
        .read_with(cx, |ws, _| (ws.all_pane_ids(), lane_count(ws)));
    change(cx).await;
    cx.update(|cx| assert!(resolve_desktop(id.0, "approve", cx)));
    cx.executor().allow_parking();
    let refused = outcome.recv().await.unwrap().unwrap();
    cx.run_until_parked();
    assert_eq!(refused, Err(ControlError::ApprovalStale), "{what}");
    let after = fixture
        .workspace
        .read_with(cx, |ws, _| (ws.all_pane_ids(), lane_count(ws)));
    assert_eq!(after, before, "{what}: nothing opened or was made");
    assert_eq!(lifecycle(task, cx), Lifecycle::Backlog, "{what}");
    cx.update(|cx| assert_eq!(guards::spent_budget_for_test(cx), spent, "{what}"));
}

/// A new worktree's task, branching from `base`, seeded in the host window.
fn new_worktree_from(
    fixture: &crate::test_support::ControlFixture,
    base: Option<std::path::PathBuf>,
    cx: &mut TestAppContext,
) -> String {
    fixture.workspace.update(cx, |ws, cx| {
        let mut spec = crate::runs::seed::spec(ws.projects[0].uuid, "Login");
        spec.base_worktree_path = base;
        crate::runs::seed::task(spec, None, cx)
    })
}

/// `task` with its base worktree and branch as `edit` leaves them.
fn relocated(task: &str, edit: impl FnOnce(&mut TaskEdit), cx: &App) -> TaskIntent {
    let spec = cx.global::<GlobalTasks>().get(task).unwrap().spec.clone();
    let mut changed = TaskEdit {
        title: spec.title,
        prompt: spec.prompt,
        notes: spec.notes,
        base_worktree_path: spec.base_worktree_path,
        branch: None,
        run_in: None,
        project: None,
        flow: spec.flow,
    };
    edit(&mut changed);
    TaskIntent::Edit {
        id: task.into(),
        edit: changed,
    }
}

/// The worktree a new one branches from, and the branch it is cut as, are
/// on the card: either edited while it waits refuses the approval.
#[gpui::test]
async fn a_task_start_whose_base_or_branch_was_edited_while_its_card_waits_is_refused(
    cx: &mut TestAppContext,
) {
    let (fixture, lane, _claude) = host(cx);
    let _connections = cx.update(Workspace::fake_agent_connections_for_test);
    let here = fixture.workspace.read_with(cx, |ws, _| {
        ws.lane_for(lane.lane_ref()).unwrap().path.clone()
    });
    type Edit = Box<dyn FnOnce(&mut TaskEdit)>;
    let edits: [(&str, Edit); 2] = [
        ("base", Box::new(|edit| edit.base_worktree_path = None)),
        (
            "branch",
            Box::new(|edit| edit.branch = Some("renamed".into())),
        ),
    ];
    for (what, edit) in edits {
        let task = new_worktree_from(&fixture, Some(here.clone()), cx);
        refused_after_change(
            &fixture,
            &task,
            what,
            async |cx: &mut TestAppContext| {
                let edited = cx.update(|cx| RunsHost::task(relocated(&task, edit, cx), cx));
                edited.await.unwrap();
            },
            cx,
        )
        .await;
    }
}

/// Set the lane at `lane` to have `branch` checked out, as a git refresh
/// does when someone switches it.
fn check_out(
    fixture: &crate::test_support::ControlFixture,
    lane: daruda_store::project::LaneRef,
    branch: &str,
    cx: &mut TestAppContext,
) {
    fixture.workspace.update(cx, |ws, _| {
        let found = ws.lane_for_mut(lane).unwrap();
        found.kind = daruda_store::project::LaneKind::Git {
            branch: Some(branch.into()),
            repo_root: found.path.clone(),
            worktree_root: found.path.clone(),
        };
    });
}

/// A card names the worktree a task runs in or branches from, which decides
/// the code its agent starts on: that worktree checking out another branch
/// while the card waits refuses the approval, with no task edited at all.
#[gpui::test]
async fn a_task_start_whose_worktree_switched_branch_while_its_card_waits_is_refused(
    cx: &mut TestAppContext,
) {
    let (fixture, lane, _claude) = host(cx);
    let _connections = cx.update(Workspace::fake_agent_connections_for_test);
    let here = fixture.workspace.read_with(cx, |ws, _| {
        ws.lane_for(lane.lane_ref()).unwrap().path.clone()
    });
    let existing = fixture.workspace.update(cx, |ws, cx| {
        let mut spec = crate::runs::seed::spec(ws.projects[0].uuid, "Login");
        spec.run_in = TaskRunIn::ExistingLane { path: here.clone() };
        crate::runs::seed::task(spec, None, cx)
    });
    let branching = new_worktree_from(&fixture, Some(here.clone()), cx);
    for (what, task) in [("existing", existing), ("new from it", branching)] {
        check_out(&fixture, lane.lane_ref(), "main", cx);
        refused_after_change(
            &fixture,
            &task,
            what,
            async |cx: &mut TestAppContext| check_out(&fixture, lane.lane_ref(), "other", cx),
            cx,
        )
        .await;
    }
}

/// The same switch, once approved: the start resolves the base branch
/// again where it reads the task and refuses before the repository lock or
/// git.
#[gpui::test]
async fn a_base_switched_as_an_approved_start_reads_its_task_refuses_it(cx: &mut TestAppContext) {
    let fx = GitFixture::open(cx, &daruda_config::Config::default(), |repo| repo);
    let _connections = cx.update(Workspace::fake_agent_connections_for_test);
    cx.executor().allow_parking();
    let (task, base) = fx.ws.update(cx, |ws, cx| {
        let base = ws.active;
        let mut spec = crate::runs::seed::spec(ws.projects[0].uuid, "Fix it");
        spec.base_worktree_path = Some(ws.lane_for(base).unwrap().path.clone());
        spec.branch_name = "switched".into();
        let checkout = crate::workspace::lane_ops::lane_checkout_path(&fx.root, "switched");
        fake_lane(&fx.fake, &checkout, "switched");
        (crate::runs::seed::task(spec, None, cx), base)
    });
    let shown = cx.update(|cx| shown_now(&fx, &task, cx));
    let answer = cx
        .update_window(fx.window.into(), |_, window, cx| {
            fx.ws.update(cx, |ws, cx| {
                if let Some(daruda_store::project::LaneKind::Git { branch, .. }) =
                    ws.lane_for_mut(base).map(|lane| &mut lane.kind)
                {
                    *branch = Some("elsewhere".into());
                } else {
                    panic!("the base is a git lane");
                }
                let submitter = crate::runs::FlowSubmitter::Orchestrator { solicited: false };
                let agent = crate::workspace::ChatAgent::Preferred(None);
                ws.control_task_start(&task, submitter, agent, Some(shown), window, cx)
            })
        })
        .unwrap();
    let refused = answer.recv().await.expect("answered");
    cx.run_until_parked();
    assert_eq!(refused, Err(ControlError::ApprovalStale));
    let added = fx
        .fake
        .calls()
        .into_iter()
        .any(|call| matches!(call, crate::lane::git::GitCall::AddLane { .. }));
    assert!(!added, "git made no worktree");
    assert_eq!(lifecycle(&task, cx), Lifecycle::Backlog);
}

/// A flow whose one agent asks for `mode`.
fn flow_asking(mode: &str) -> String {
    format!(
        "version: 1\ndefaults:\n  agent:\n    id: claude\nnodes:\n  - id: a\n    kind: agent\n    agent:\n      mode: {mode}\n    output: a.md\n    prompt: p\n"
    )
}

/// A flow task's card names the modes its flow file gives its agents: that
/// file rewritten while the card waits refuses the approval, though the task
/// still names the same flow.
#[gpui::test]
async fn a_task_start_whose_flow_file_changed_while_its_card_waits_is_refused(
    cx: &mut TestAppContext,
) {
    let (fixture, lane, _claude) = host(cx);
    let here = fixture.workspace.read_with(cx, |ws, _| {
        ws.lane_for(lane.lane_ref()).unwrap().path.clone()
    });
    let flows = crate::workspace::flow_paths::flows_dir(&here);
    std::fs::create_dir_all(&flows).unwrap();
    let file = flows.join("ship.yaml");
    std::fs::write(&file, flow_asking("plan")).unwrap();
    let task = fixture.workspace.update(cx, |ws, cx| {
        let mut spec = crate::runs::seed::spec(ws.projects[0].uuid, "Ship");
        spec.run_in = TaskRunIn::ExistingLane { path: here.clone() };
        spec.flow = Some(daruda_store::tasks::TaskFlow {
            name: "ship".into(),
            profile: None,
            input: None,
        });
        crate::runs::seed::task(spec, None, cx)
    });
    let rewritten = file.clone();
    refused_after_change(
        &fixture,
        &task,
        "flow content",
        async move |_: &mut TestAppContext| {
            std::fs::write(&rewritten, flow_asking("bypassPermissions")).unwrap()
        },
        cx,
    )
    .await;
    // The card read the file it pinned: it named the mode the file had then.
    let (_id, _outcome, card) = ask(GatedCommand::TaskStart { task }, cx);
    assert!(card.contains("bypassPermissions"), "{card}");
}
