//! Right-panel Tasks tab — workspace-side behaviour tests.
//!
//! Covers the lifecycle paths whose correctness depends on
//! `Workspace` state (hook mapping, starts, lane focus). Pure data-model
//! checks live next to the data in `daruda_store::tasks::tests`.

use super::{build_workspace, fake_lane, fake_repo};

use daruda_store::tasks::machine::ErrorCause;
use daruda_store::tasks::{CliProcessState, Lifecycle, SessionEndReason};
use gpui::{AppContext as _, TestAppContext};

use crate::runs::GlobalTasks;
use crate::runs::seed::{SeedEnd, SeedRun};

#[gpui::test]
fn illegal_task_commands_report_without_changing_state(cx: &mut TestAppContext) {
    let (_window, workspace) = build_workspace(cx);
    let (done, running) = workspace.update(cx, |_ws, cx| {
        let run = SeedRun::chat("/wt");
        let done = crate::runs::seed::task(
            crate::runs::seed::spec(Default::default(), "done"),
            Some((&run, SeedEnd::Done)),
            cx,
        );
        let running = crate::runs::seed::task(
            crate::runs::seed::spec(Default::default(), "running"),
            Some((&run, SeedEnd::Running)),
            cx,
        );
        (done, running)
    });
    workspace.update(cx, |ws, cx| {
        ws.cancel_task(&done, cx);
        ws.reopen_task(&running, cx);
    });
    cx.run_until_parked();
    assert!(matches!(lifecycle(&done, cx), Lifecycle::Done { .. }));
    assert!(matches!(lifecycle(&running, cx), Lifecycle::Running { .. }));
    workspace.read_with(cx, |ws, _| {
        assert_eq!(ws.error_history().len(), 2);
        assert!(
            ws.error_history()
                .iter()
                .all(|report| report.dedup_key.as_deref() == Some("tasks.command_refused"))
        );
    });
}

/// A running CLI task with `sessions` attached; answers its id.
fn seed_cli_task(
    workspace: &gpui::Entity<crate::workspace::Workspace>,
    cx: &mut TestAppContext,
    session_id: Option<&str>,
    sessions: &[&str],
) -> String {
    workspace.update(cx, |_ws, cx| {
        let spec =
            crate::runs::seed::spec(daruda_store::project::ProjectUuid::default(), "fix-bug");
        let mut run = SeedRun::cli("/tmp/wt", CliProcessState::Running { pid: 1 });
        if let Some(session) = session_id {
            run = run.session(session);
        }
        for session in sessions {
            run = run.attach(*session);
        }
        crate::runs::seed::task(spec, Some((&run, SeedEnd::Running)), cx)
    })
}

/// Insert a running task with one attached session and return the task
/// id. Fixture for the task-deletion test.
fn seed_running_task(
    workspace: &gpui::Entity<crate::workspace::Workspace>,
    cx: &mut TestAppContext,
    session_id: &str,
) -> String {
    seed_cli_task(workspace, cx, Some(session_id), &[session_id])
}

fn lifecycle(id: &str, cx: &mut TestAppContext) -> Lifecycle {
    cx.update(|cx| {
        cx.global::<GlobalTasks>()
            .get(id)
            .unwrap()
            .lifecycle
            .clone()
    })
}

/// Deleting a running task removes it. The task machine accepts `Deleted`
/// only from Backlog or a terminal lifecycle, so its run had to end first.
#[gpui::test]
fn deleting_a_running_task_ends_its_run_and_removes_it(cx: &mut TestAppContext) {
    let (wh, workspace) = build_workspace(cx);
    let task_id = seed_running_task(&workspace, cx, "sess-delete");
    cx.update_window(wh.into(), |_, window, cx| {
        workspace.update(cx, |ws, cx| ws.delete_task(&task_id, window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        assert!(
            cx.global::<GlobalTasks>().get(&task_id).is_none(),
            "a running task ends, then is deleted"
        );
    });
}

#[test]
fn classify_hook_end_reason_maps_known_events() {
    assert_eq!(
        crate::runs::cli_hook_end_reason("Stop"),
        Some(SessionEndReason::Stop),
    );
    assert_eq!(
        crate::runs::cli_hook_end_reason("SessionEnd"),
        Some(SessionEndReason::Other),
    );
    assert_eq!(
        crate::runs::cli_hook_end_reason("PreToolUse"),
        None,
        "non-terminal events must not transition tasks",
    );
    assert_eq!(
        crate::runs::cli_hook_end_reason("PostToolUseFailure"),
        None,
        "a failed tool call ends nothing",
    );
}

#[gpui::test]
fn apply_task_session_changed_attaches_idempotently_then_error_ends(cx: &mut TestAppContext) {
    let (_wh, workspace) = build_workspace(cx);
    // Attachment follows the run's recorded session, never the cwd alone.
    let id = seed_cli_task(&workspace, cx, Some("sess-2"), &[]);
    workspace.update(cx, |ws, cx| {
        ws.apply_task_session_changed("sess-other", cx);
        ws.apply_task_session_changed("sess-2", cx);
    });
    cx.run_until_parked();
    workspace.update(cx, |ws, cx| ws.apply_task_session_changed("sess-2", cx));
    cx.run_until_parked();
    cx.update(|cx| {
        let t = cx.global::<GlobalTasks>().get(&id).unwrap();
        assert_eq!(t.session_ids.as_slice(), &["sess-2".to_string()]);
    });

    workspace.update(cx, |ws, cx| {
        ws.apply_task_session_ended("sess-2", SessionEndReason::Error, cx);
    });
    cx.run_until_parked();
    match lifecycle(&id, cx) {
        Lifecycle::Error { cause, .. } => assert_eq!(cause, ErrorCause::SessionFailed),
        other => panic!("expected Error, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Subtask ops
// ---------------------------------------------------------------------------

/// Insert a Backlog task and return its id. Subtask tests don't need a
/// session or lane.
fn seed_backlog_task(
    workspace: &gpui::Entity<crate::workspace::Workspace>,
    cx: &mut TestAppContext,
) -> String {
    workspace.update(cx, |_ws, cx| {
        let spec = crate::runs::seed::spec(
            daruda_store::project::ProjectUuid::default(),
            "subtask-host",
        );
        crate::runs::seed::task(spec, None, cx)
    })
}

fn subtasks(id: &str, cx: &mut TestAppContext) -> Vec<daruda_store::tasks::SubTask> {
    cx.update(|cx| cx.global::<GlobalTasks>().get(id).unwrap().subtasks.clone())
}

#[gpui::test]
fn subtask_ops_add_toggle_rename_and_delete(cx: &mut TestAppContext) {
    let (_wh, workspace) = build_workspace(cx);
    let task_id = seed_backlog_task(&workspace, cx);

    workspace.update(cx, |ws, cx| {
        ws.add_subtask(&task_id, "first".into(), cx);
        ws.add_subtask(&task_id, "   ".into(), cx); // whitespace → no-op
        ws.add_subtask(&task_id, "  second  ".into(), cx); // trimmed
    });
    cx.run_until_parked();
    let t = subtasks(&task_id, cx);
    assert_eq!(t.len(), 2);
    assert_eq!(t[0].title, "first");
    assert_eq!(t[1].title, "second");
    assert!(t.iter().all(|s| s.source_session_id.is_none()));
    let (first_id, second_id) = (t[0].id.clone(), t[1].id.clone());

    workspace.update(cx, |ws, cx| ws.toggle_subtask(&task_id, &first_id, cx));
    cx.run_until_parked();
    assert!(subtasks(&task_id, cx)[0].completed);
    workspace.update(cx, |ws, cx| ws.toggle_subtask(&task_id, &first_id, cx));
    cx.run_until_parked();
    assert!(!subtasks(&task_id, cx)[0].completed);

    workspace.update(cx, |ws, cx| {
        ws.rename_subtask(&task_id, &second_id, "  new title  ".into(), cx)
    });
    cx.run_until_parked();
    assert_eq!(
        subtasks(&task_id, cx)[1].title,
        "new title",
        "rename trims whitespace"
    );
    workspace.update(cx, |ws, cx| {
        ws.rename_subtask(&task_id, &second_id, "   ".into(), cx)
    });
    cx.run_until_parked();
    assert_eq!(
        subtasks(&task_id, cx)[1].title,
        "new title",
        "empty rename is ignored"
    );

    workspace.update(cx, |ws, cx| ws.delete_subtask(&task_id, &first_id, cx));
    cx.run_until_parked();
    let t = subtasks(&task_id, cx);
    assert_eq!(t.len(), 1);
    assert_eq!(t[0].title, "new title");
}

/// A task's worktree is made in the repository of the project it belongs
/// to, not in whichever project the window happens to be showing.
#[gpui::test]
fn a_task_starts_in_its_own_project_while_another_is_active(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let fake = crate::test_support::fake_git(cx);
    let a_root = fake_repo(&fake, &temp.path().join("a"));
    let b_root = fake_repo(&fake, &temp.path().join("b"));
    let (window, ws) = super::build_workspace_with(
        cx,
        &daruda_config::Config::default(),
        Some(daruda_store::project::Project::from_path(&a_root)),
    );
    ws.update(cx, |ws, cx| ws.reconcile_bootstrapped_lanes(cx));
    cx.run_until_parked();
    let _connections = cx.update(super::Workspace::fake_agent_connections_for_test);
    let (task_id, expected, rx) = cx
        .update_window(window.into(), |_, window, cx| {
            ws.update(cx, |ws, cx| {
                let a = &ws.projects[0];
                let (a_id, a_uuid) = (a.id, a.uuid);
                ws.add_project(b_root.clone(), window, cx);
                assert_ne!(ws.active.project, a_id, "project B is the active one");
                let task = crate::runs::seed::spec(a_uuid, "In A");
                let expected = crate::workspace::lane_ops::lane_checkout_path(
                    &ws.git_repo_root_for_project(a_id)
                        .expect("A is a git project"),
                    &task.branch_name,
                );
                fake_lane(&fake, &expected, &task.branch_name);
                let id = crate::runs::seed::task(task, None, cx);
                let rx =
                    ws.begin_task_start(&id, daruda_store::tasks::StartedBy::Desktop, window, cx);
                assert!(rx.try_recv().is_err(), "git has yet to run");
                (id, expected, rx)
            })
        })
        .unwrap();
    cx.run_until_parked();
    let started = rx
        .try_recv()
        .expect("answered once git ran")
        .expect("started");
    assert_eq!(started.worktree, expected);
    let added = crate::test_support::added_lane(&fake);
    assert_eq!(
        added.repo, a_root,
        "the worktree is added in A's repository"
    );
    assert_eq!(added.new_path, expected);
    ws.read_with(cx, |_, cx| {
        let task = cx.global::<GlobalTasks>().get(&task_id).unwrap();
        assert_eq!(
            task.lifecycle.worktree(),
            Some(expected.as_path()),
            "{:?}",
            task.lifecycle
        );
        assert!(matches!(task.lifecycle, Lifecycle::Running { .. }));
    });
}

/// Focusing a task's lane reaches it in whichever project holds it — the
/// active project need not be the task's.
#[gpui::test]
fn focusing_a_task_lane_crosses_projects(cx: &mut TestAppContext) {
    let a_root = tempfile::tempdir().unwrap();
    let b_root = tempfile::tempdir().unwrap();
    let config = daruda_config::Config::default();
    let project = daruda_store::project::Project::from_path(a_root.path());
    let (wh, workspace) = super::build_workspace_with(cx, &config, Some(project));
    cx.update_window(wh.into(), |_, window, cx| {
        workspace.update(cx, |ws, cx| {
            let a = ws.active;
            let a_path = ws.lane_for(a).unwrap().path.clone();
            let spec = crate::runs::seed::spec(ws.projects[0].uuid, "elsewhere");
            let id =
                crate::runs::seed::task(spec, Some((&SeedRun::chat(a_path), SeedEnd::Running)), cx);
            ws.add_project(b_root.path().to_path_buf(), window, cx);
            assert_ne!(ws.active.project, a.project, "the other project is active");
            ws.focus_task_lane(&id, window, cx);
            assert_eq!(ws.active, a, "the task's own lane, in its own project");
        });
    })
    .unwrap();
}

/// Start an Agent-chat task as a new worktree from `started_by`, with the
/// orchestrator's tab shown, and report the window before and after.
fn start_new_worktree_task(
    started_by: daruda_store::tasks::StartedBy,
    cx: &mut TestAppContext,
) -> NewWorktreeStart {
    let fx = super::GitFixture::open(cx, &daruda_config::Config::default(), |repo| repo);
    let (window, ws, fake, root) = (fx.window, &fx.ws, &fx.fake, &fx.root);
    let probes = cx.update(super::Workspace::fake_agent_connections_for_test);
    let (task_id, before, orchestrator, rx) = cx
        .update_window(window.into(), |_, window, cx| {
            ws.update(cx, |ws, cx| {
                let orchestrator = ws
                    .seed_orchestrator_chat_pane_unrevealed_for_test(
                        ws.mirrors.agents[0].id.clone(),
                        std::env::temp_dir(),
                        daruda_store::accounts::AccountSelection::SystemDefault,
                        None,
                        window,
                        cx,
                    )
                    .unwrap();
                assert!(ws.show_orchestrator_tab(window, cx));
                let task = crate::runs::seed::spec(ws.projects[0].uuid, "Elsewhere");
                let checkout =
                    crate::workspace::lane_ops::lane_checkout_path(root, &task.branch_name);
                fake_lane(fake, &checkout, &task.branch_name);
                let id = crate::runs::seed::task(task, None, cx);
                let rx = ws.begin_task_start(&id, started_by, window, cx);
                (id, ws.active, orchestrator, rx)
            })
        })
        .unwrap();
    cx.run_until_parked();
    let started = rx.try_recv().expect("answered").expect("started");
    assert_eq!(
        crate::test_support::added_lane(fake).new_path,
        started.worktree
    );
    let lifecycle = ws.read_with(cx, |_, cx| {
        cx.global::<GlobalTasks>()
            .get(&task_id)
            .unwrap()
            .lifecycle
            .clone()
    });
    assert!(
        matches!(lifecycle, Lifecycle::Running { .. }),
        "{lifecycle:?}"
    );
    assert_eq!(probes.borrow().len(), 1, "the task's session connected");
    let pane = started.chat().expect("an Agent chat run");
    let requested = cx.update(super::Workspace::faked_connect_preferences_for_test);
    assert_eq!(requested.len(), 1);
    assert_eq!(requested[0].0, pane);
    let modes = requested[0].1.modes.clone();
    ws.read_with(cx, |ws, cx| {
        let lane = ws.lane_ref_at(&started.worktree).expect("lane registered");
        let rt = ws.main_area.runtimes.get(&lane).expect("lane runtime");
        assert!(rt.panes.iter().any(|p| p.id == pane));
        assert_eq!(
            ws.agent_chat_view(pane).unwrap().read(cx).status(),
            &daruda_chat::view::AgentSessionStatus::Connected,
        );
        let active = ws.active_runtime();
        NewWorktreeStart {
            before,
            after: ws.active,
            lane,
            orchestrator_in_front: active
                .tabs
                .get(active.active_tab_index)
                .is_some_and(|tab| ws.is_orchestrator_tab(tab))
                && active.focused_pane_id == orchestrator,
            modes,
        }
    })
}

struct NewWorktreeStart {
    before: daruda_store::project::LaneRef,
    after: daruda_store::project::LaneRef,
    lane: daruda_store::project::LaneRef,
    orchestrator_in_front: bool,
    /// What the task's chat asked its agent for on its first connect.
    modes: Vec<String>,
}

/// A worktree the orchestrator started a task in is made and its chat runs,
/// but the window stays on the orchestrator's tab the person is using.
#[gpui::test]
fn an_orchestrator_started_worktree_does_not_take_the_window(cx: &mut TestAppContext) {
    let start = start_new_worktree_task(daruda_store::tasks::StartedBy::Orchestrator, cx);
    assert_ne!(start.lane, start.before);
    assert_eq!(start.after, start.before, "the active lane stays");
    assert!(start.orchestrator_in_front);
    assert_eq!(
        start.modes,
        vec!["bypassPermissions".to_string()],
        "its first connect already asks for the delegated mode"
    );
}

/// Started from this window's Tasks page, the new worktree is brought up.
#[gpui::test]
fn a_desktop_started_worktree_becomes_the_active_lane(cx: &mut TestAppContext) {
    let start = start_new_worktree_task(daruda_store::tasks::StartedBy::Desktop, cx);
    assert_eq!(start.after, start.lane);
    assert!(!start.orchestrator_in_front);
    assert!(
        start.modes.is_empty(),
        "a desktop start follows default_mode"
    );
}
