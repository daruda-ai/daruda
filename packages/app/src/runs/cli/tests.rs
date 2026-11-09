mod wait_tests;
use super::*;
use crate::runs::seed::{self, SeedEnd, SeedRun};
use daruda_agent::hooks::status_file::StatusFile;
use daruda_store::tasks::{Lifecycle, SessionEndReason};
use gpui::TestAppContext;

/// A CLI run an earlier launch left running as `pid`: the terminal it
/// recorded belonged to that launch, so no pane of this one owns it.
fn old_run(session: &str, pid: u32) -> SeedRun {
    let mut run = SeedRun::cli("/cli", CliProcessState::Running { pid }).session(session);
    run.terminal_id = Some(
        crate::hooks::agent_hooks::mint_terminal_id()
            .as_str()
            .to_owned(),
    );
    run
}

/// A terminal of this launch, registered as a pane's live shell.
fn live_terminal(cx: &mut gpui::App) -> TerminalId {
    let terminal = crate::hooks::agent_hooks::mint_terminal_id();
    let pane = RunPane {
        workspace: WorkspaceUuid::new(),
        pane: 1,
    };
    RunsHost::register_terminal(pane, terminal.clone(), cx);
    terminal
}

fn hook(session: &str, event: &str, terminal: TerminalId, pid: u32) -> StatusFile {
    let mut file = StatusFile::new_hook(session, "/cli", daruda_agent::SessionStatus::Idle, event);
    file.terminal_id = Some(terminal);
    file.agent_pid = Some(pid);
    file
}

#[gpui::test]
fn cli_result_read_protects_the_execution_from_exit_reconciliation(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("result.jsonl");
    std::fs::write(
        &path,
        super::super::cli_result::tests::assistant("completed CLI answer", false),
    )
    .unwrap();
    let store = daruda_store::tasks::TaskStore::in_directory(
        dir.path().join("task-log"),
        None,
        "test",
        super::super::no_hook(),
    )
    .unwrap();
    let (id, settled_by) = cx.update(|cx| {
        RunsHost::install(RunsHost::open(Ok(store.clone())), cx);
        let run = SeedRun::cli("/cli", CliProcessState::Running { pid: 42 }).session("s");
        let id = seed::task(
            seed::spec(Default::default(), "CLI"),
            Some((&run, SeedEnd::Running)),
            cx,
        );
        seed::commit_now(
            vec![TaskEvent::CliTranscriptFound {
                exec: run.exec(&id),
                transcript_path: path.clone(),
                at: Utc::now(),
            }],
            cx,
        );
        RunsHost::settle_cli_session("s", SessionEndReason::Stop, cx);
        let settled_by = Utc::now();
        assert!(
            cx.global::<RunsHost>()
                .executions
                .settling
                .contains(&run.exec(&id))
        );
        RunsHost::record_cli_exit("s", 42, cx);
        assert!(matches!(
            cx.global::<GlobalTasks>().get(&id).unwrap().lifecycle,
            Lifecycle::Running { .. }
        ));
        // The transcript read resolves later; its latency is not run time.
        std::thread::sleep(std::time::Duration::from_millis(20));
        (id, settled_by)
    });
    cx.run_until_parked();
    cx.update(|cx| {
        let task = cx.global::<GlobalTasks>().get(&id).unwrap();
        assert!(matches!(task.lifecycle, Lifecycle::Done { .. }));
        assert_eq!(
            task.ended_executions.last().unwrap().result.as_deref(),
            Some("completed CLI answer")
        );
        assert!(task.ended_executions.last().unwrap().finished_at <= settled_by);
        assert!(cx.global::<RunsHost>().executions.settling.is_empty());
        assert_eq!(store.current().unwrap().1.get(&id), Some(task));
    });
}

#[gpui::test]
fn a_cli_result_read_cannot_settle_a_restarted_execution(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("answer.jsonl");
    std::fs::write(
        &path,
        super::super::cli_result::tests::assistant("old answer", false),
    )
    .unwrap();
    let (id, replacement) = cx.update(|cx| {
        RunsHost::inline_commits_for_test(cx);
        let run = SeedRun::cli("/cli", CliProcessState::Running { pid: 42 }).session("old");
        let id = seed::task(
            seed::spec(Default::default(), "CLI"),
            Some((&run, SeedEnd::Running)),
            cx,
        );
        RunsHost::record_cli_transcript("old", &path, cx);
        RunsHost::settle_cli_session("old", SessionEndReason::Stop, cx);
        seed::commit_now(
            vec![
                TaskEvent::CancelRequested {
                    exec: run.exec(&id),
                    at: Utc::now(),
                },
                TaskEvent::Reopened {
                    id: id.clone(),
                    run_in: Default::default(),
                    at: Utc::now(),
                },
            ],
            cx,
        );
        let replacement = SeedRun::cli("/cli", CliProcessState::Running { pid: 43 });
        seed::commit_now(seed::run(&id, &replacement, SeedEnd::Running), cx);
        (id, replacement)
    });
    cx.run_until_parked();
    cx.update(|cx| {
        let record = cx.global::<GlobalTasks>().get(&id).unwrap();
        assert!(matches!(&record.lifecycle, Lifecycle::Running { exec, .. } if exec == &replacement.exec(&id)));
        assert_eq!(record.ended_executions.len(), 1);
        assert_eq!(record.ended_executions[0].result, None);
        assert!(cx.global::<RunsHost>().executions.settling.is_empty());
    });
}

#[gpui::test]
fn an_observed_exit_is_recorded_durably(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let store = daruda_store::tasks::TaskStore::in_directory(
        directory.path(),
        None,
        "test",
        super::super::no_hook(),
    )
    .unwrap();
    let id = cx.update(|cx| {
        RunsHost::install(RunsHost::open(Ok(store.clone())), cx);
        let run = old_run("session", 42);
        let id = seed::task(
            seed::spec(Default::default(), "CLI"),
            Some((&run, SeedEnd::Running)),
            cx,
        );
        RunsHost::record_cli_exit("session", 42, cx);
        id
    });
    cx.run_until_parked();
    let durable = store.current().unwrap().1;
    let process = durable
        .get(&id)
        .unwrap()
        .execution
        .as_ref()
        .unwrap()
        .source
        .cli_process();
    assert!(
        matches!(process, Some(CliProcessState::ExitConfirmed { .. })),
        "{process:?}"
    );
    cx.update(|cx| assert_eq!(cx.global::<GlobalTasks>().get(&id), durable.get(&id)));
}

#[gpui::test]
fn the_tracker_keeps_both_windows_panes_distinct(cx: &mut TestAppContext) {
    cx.update(|cx| {
        RunsHost::ensure_installed(cx);
        let first = RunPane {
            workspace: WorkspaceUuid::new(),
            pane: 1,
        };
        let second = RunPane {
            workspace: WorkspaceUuid::new(),
            pane: 1,
        };
        RunsHost::register_terminal(first, crate::hooks::agent_hooks::mint_terminal_id(), cx);
        RunsHost::register_terminal(second, crate::hooks::agent_hooks::mint_terminal_id(), cx);
        assert_ne!(
            RunsHost::terminal_id(first, cx),
            RunsHost::terminal_id(second, cx)
        );
        RunsHost::release_workspace(first.workspace, cx);
        assert!(RunsHost::terminal_id(first, cx).is_none());
        assert!(RunsHost::terminal_id(second, cx).is_some());
    });
}

#[gpui::test]
fn a_restored_cli_without_a_live_process_does_not_stay_running(cx: &mut TestAppContext) {
    cx.update(|cx| {
        RunsHost::inline_commits_for_test(cx);
        let run = SeedRun::cli("/cli", CliProcessState::Running { pid: 42 }).session("restored");
        let id = seed::task(
            seed::spec(Default::default(), "CLI"),
            Some((&run, SeedEnd::Running)),
            cx,
        );
        RunsHost::record_cli_exit("restored", 42, cx);
        assert!(matches!(
            cx.global::<GlobalTasks>().get(&id).unwrap().lifecycle,
            Lifecycle::Interrupted {
                cause: daruda_store::tasks::machine::InterruptCause::AppRestart,
                ..
            }
        ));
        let seq = RunsHost::sequence_for_test(cx);
        RunsHost::record_cli_exit("restored", 42, cx);
        assert_eq!(RunsHost::sequence_for_test(cx), seq);
    });
}

#[gpui::test]
fn releasing_a_replaced_view_keeps_its_successor_registered(cx: &mut TestAppContext) {
    cx.update(|cx| {
        RunsHost::ensure_installed(cx);
        let pane = RunPane {
            workspace: WorkspaceUuid::new(),
            pane: 1,
        };
        let old = crate::hooks::agent_hooks::mint_terminal_id();
        let fresh = crate::hooks::agent_hooks::mint_terminal_id();
        RunsHost::register_terminal(pane, old.clone(), cx);
        RunsHost::register_terminal(pane, fresh.clone(), cx);
        RunsHost::release_terminal(pane, &old, cx);
        assert_eq!(RunsHost::terminal_pane(&fresh, cx), Some(pane));
        assert!(RunsHost::terminal_pane(&old, cx).is_none());
        RunsHost::release_terminal(pane, &fresh, cx);
        assert!(RunsHost::terminal_pane(&fresh, cx).is_none());
    });
}

#[gpui::test]
fn a_killed_cli_that_outlived_its_launch_does_not_stay_running(cx: &mut TestAppContext) {
    let child = test_process::sleeping();
    let pid = child.id();
    let (tracker, exits) = PtyTracker::spawn();
    let id = cx.update(|cx| {
        RunsHost::inline_commits_for_test(cx);
        let run = old_run("killed-session", pid);
        let id = seed::task(
            seed::spec(Default::default(), "CLI"),
            Some((&run, SeedEnd::Running)),
            cx,
        );
        tracker.track_agent("killed-session".into(), pid);
        id
    });
    drop(child);
    tracker.poke();
    let exit = exits.recv_timeout(Duration::from_secs(5)).unwrap();
    cx.update(|cx| {
        RunsHost::apply_cli_tracker_event(exit, cx);
        let task = cx.global::<GlobalTasks>().get(&id).unwrap();
        assert!(matches!(
            task.lifecycle,
            Lifecycle::Interrupted {
                cause: daruda_store::tasks::machine::InterruptCause::AppRestart,
                ..
            }
        ));
        assert!(matches!(
            task.execution.as_ref().unwrap().source.cli_process(),
            Some(CliProcessState::ExitConfirmed { .. })
        ));
        assert_eq!(task.ended_executions.len(), 1);
        assert_eq!(
            task.execution
                .as_ref()
                .unwrap()
                .cli_session_end
                .as_ref()
                .unwrap()
                .reason,
            daruda_store::tasks::CliSessionEndReason::AppRestart
        );
    });
}

#[gpui::test]
fn a_normal_exit_preserves_the_hook_completed_run(cx: &mut TestAppContext) {
    let mut child = daruda_core::process::command(test_process::executable())
        .args(["--sleep-ms", "100"])
        .spawn()
        .unwrap();
    let pid = child.id();
    let (tracker, exits) = PtyTracker::spawn();
    let id = cx.update(|cx| {
        RunsHost::inline_commits_for_test(cx);
        let run = old_run("normal-session", pid);
        let id = seed::task(
            seed::spec(Default::default(), "CLI"),
            Some((&run, SeedEnd::Running)),
            cx,
        );
        tracker.track_agent("normal-session".into(), pid);
        RunsHost::settle_cli_session("normal-session", SessionEndReason::Stop, cx);
        id
    });
    cx.run_until_parked();
    assert!(child.wait().unwrap().success());
    tracker.poke();
    let exit = exits.recv_timeout(Duration::from_secs(5)).unwrap();
    cx.update(|cx| {
        RunsHost::apply_cli_tracker_event(exit, cx);
        let task = cx.global::<GlobalTasks>().get(&id).unwrap();
        assert!(matches!(task.lifecycle, Lifecycle::Done { .. }));
        assert_eq!(task.ended_executions.len(), 1);
        assert!(matches!(
            task.execution.as_ref().unwrap().source.cli_process(),
            Some(CliProcessState::ExitConfirmed { .. })
        ));
    });
}

#[gpui::test]
fn a_persisted_stop_wins_when_the_exit_arrives_before_the_watcher(cx: &mut TestAppContext) {
    for event in ["Stop", "SessionEnd", "StopFailure"] {
        cx.update(|cx| {
            RunsHost::inline_commits_for_test(cx);
            // The old terminal's final hook, written by the process that
            // survived the restart; no pane of this launch owns its route.
            let run = old_run("unread-final-hook", 42);
            let id = seed::task(
                seed::spec(Default::default(), "CLI"),
                Some((&run, SeedEnd::Running)),
                cx,
            );
            let terminal = TerminalId::parse(run.terminal_id.as_deref().unwrap()).unwrap();
            assert!(RunsHost::terminal_pane(&terminal, cx).is_none());
            let file = hook("unread-final-hook", event, terminal, 42);
            let dir = RunsHost::status_dir(cx).unwrap();
            let path = daruda_agent::hooks::status_file::path_for(dir, file.storage_key());
            daruda_agent::hooks::status_file::write_atomic(&path, &file).unwrap();
            RunsHost::record_cli_exit("unread-final-hook", 42, cx);
            let task = cx.global::<GlobalTasks>().get(&id).unwrap();
            if event == "StopFailure" {
                assert!(
                    matches!(task.lifecycle, Lifecycle::Error { .. }),
                    "{event}: {:?}",
                    task.lifecycle
                );
            } else {
                assert!(
                    matches!(task.lifecycle, Lifecycle::Done { .. }),
                    "{event}: {:?}",
                    task.lifecycle
                );
            }
            assert_eq!(task.ended_executions.len(), 1);
            std::fs::remove_file(path).unwrap();
        });
    }
}

#[gpui::test]
fn ending_one_old_terminal_does_not_settle_another_on_its_session(cx: &mut TestAppContext) {
    cx.update(|cx| {
        RunsHost::inline_commits_for_test(cx);
        let mut ids = Vec::new();
        for pid in [41, 42] {
            let run = old_run("shared-session", pid);
            let id = seed::task(
                seed::spec(Default::default(), "CLI"),
                Some((&run, SeedEnd::Running)),
                cx,
            );
            ids.push((id, run.terminal_id));
        }
        RunsHost::settle_cli_terminal(
            "shared-session",
            ids[0].1.clone(),
            SessionEndReason::Stop,
            cx,
        );
        assert!(matches!(
            cx.global::<GlobalTasks>().get(&ids[0].0).unwrap().lifecycle,
            Lifecycle::Done { .. }
        ));
        let second = cx.global::<GlobalTasks>().get(&ids[1].0).unwrap();
        assert!(matches!(second.lifecycle, Lifecycle::Running { .. }));
        assert_eq!(
            second.execution.as_ref().unwrap().source.cli_process(),
            Some(&CliProcessState::Running { pid: 42 })
        );
    });
}

/// A running run's terminal belonged to an earlier launch, so a live
/// terminal resuming its session reports neither its process nor its end.
#[gpui::test]
fn a_live_terminal_on_a_running_runs_session_leaves_it_alone(cx: &mut TestAppContext) {
    cx.update(|cx| {
        RunsHost::inline_commits_for_test(cx);
        let run = old_run("taken-session", 41);
        let id = seed::task(
            seed::spec(Default::default(), "CLI"),
            Some((&run, SeedEnd::Running)),
            cx,
        );
        let seq = RunsHost::sequence_for_test(cx);
        let terminal = live_terminal(cx);
        RunsHost::record_task_cli_hook(&hook("taken-session", "SessionEnd", terminal, 77), cx);
        assert_eq!(RunsHost::sequence_for_test(cx), seq);
        let task = cx.global::<GlobalTasks>().get(&id).unwrap();
        let execution = task.execution.as_ref().unwrap();
        assert!(matches!(task.lifecycle, Lifecycle::Running { .. }));
        assert_eq!(
            execution.source.cli_process(),
            Some(&CliProcessState::Running { pid: 41 })
        );
        assert_eq!(execution.cli_session_end, None);
    });
}

/// An ended run's session resumed in a terminal of this launch is being
/// written again: the run learns the new process.
#[gpui::test]
fn a_live_terminal_resuming_an_ended_runs_session_reports_its_process(cx: &mut TestAppContext) {
    cx.update(|cx| {
        RunsHost::inline_commits_for_test(cx);
        let run = old_run("resumed-session", 41);
        let id = seed::task(
            seed::spec(Default::default(), "CLI"),
            Some((&run, SeedEnd::Running)),
            cx,
        );
        RunsHost::record_cli_exit("resumed-session", 41, cx);
        assert!(!matches!(
            cx.global::<GlobalTasks>().get(&id).unwrap().lifecycle,
            Lifecycle::Running { .. }
        ));
        let terminal = live_terminal(cx);
        RunsHost::record_task_cli_hook(&hook("resumed-session", "SessionStart", terminal, 77), cx);
        assert_eq!(
            cx.global::<GlobalTasks>()
                .get(&id)
                .unwrap()
                .execution
                .as_ref()
                .unwrap()
                .source
                .cli_process(),
            Some(&CliProcessState::Running { pid: 77 })
        );
    });
}
