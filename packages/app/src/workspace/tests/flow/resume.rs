//! `daruda_flow_resume`: the orchestrator picking up a flow run the app's
//! quit or a crash cut off, after the person approves, and the refusals it
//! answers before any card is shown.

use super::*;

use crate::control::approval::{self, ApprovalChoice, resolve_only_pending_for_test};
use crate::control::exec::{self, CommandOrigin, Dispatch};
use crate::control::result::{
    ControlError, ControlOutcome, ControlResult, LaneHandle, NotResumableReason,
};
use crate::control::spec::{GatedCommand, ResolvedCommand, ResolvedFlowCommand};
use crate::runs::RunsHost;
use crate::runs::result::{EndedAs, RunKey, RunState};
use crate::workspace::flow_paths;
use daruda_control_types::runs::RunHandle;
use daruda_flow::request::StartedBy;

/// Two steps, each leaving a line behind so a test can count its runs. The
/// second waits while `go` is missing, which is how a test holds it open.
const TWO_STEPS: &str = "\
version: 1
nodes:
  - id: first
    kind: command
    run: \"echo ran >> first.log\"
  - id: second
    kind: command
    deps: [first]
    run: \"test -f go || sleep 30; echo ran >> second.log\"
";

/// An orchestrator-started run that a crash cut off right after `first`
/// passed, in a window the control surface can reach.
struct Interrupted {
    lane: tempfile::TempDir,
    ws: gpui::Entity<Workspace>,
    run_id: String,
    run_dir: std::path::PathBuf,
    orchestrator: gpui::Entity<daruda_chat::view::AgentChatView>,
}

impl Interrupted {
    fn lines(&self, file: &str) -> usize {
        std::fs::read_to_string(self.lane.path().join(file)).map_or(0, |t| t.lines().count())
    }

    fn handle(&self) -> RunHandle {
        RunHandle::Flow {
            run_id: self.run_id.clone(),
        }
    }

    fn resume(&self) -> GatedCommand {
        GatedCommand::FlowResume { run: self.handle() }
    }
}

/// Start [`TWO_STEPS`] as the orchestrator, let it finish, then rewrite what
/// it left into what a crash after `first` leaves: a log ending there, no
/// marker, and a lock naming a pid that is gone (0 never lives).
async fn interrupted_orchestrator_run(cx: &mut TestAppContext) -> Interrupted {
    interrupted_run(CommandOrigin::Orchestrator, "first", cx).await
}

/// [`interrupted_orchestrator_run`], started by `origin` and cut off right
/// after `last` passed.
async fn interrupted_run(
    origin: CommandOrigin,
    last: &str,
    cx: &mut TestAppContext,
) -> Interrupted {
    let (lane, ws, _flow, _wh) = workspace_with_a_flow(cx, TWO_STEPS);
    let pane = crate::test_support::register_test_orchestrator(cx);
    let orchestrator = cx.update(|cx| {
        crate::window_registry::WindowRegistry::orchestrator(cx)
            .unwrap()
            .1
            .upgrade()
            .unwrap()
            .read(cx)
            .agent_chat_view(pane.pane)
            .unwrap()
            .clone()
    });
    std::fs::write(lane.path().join("go"), "").expect("go");
    // A real run: its log thread wakes the app from outside the scheduler.
    cx.executor().allow_parking();
    let started = cx.update(|cx| {
        let target = ws.read(cx);
        exec::run(
            ResolvedCommand::Flow(ResolvedFlowCommand::Run {
                name: "ship".into(),
                lane: LaneHandle::new(target.uuid(), target.active),
                profile: None,
                inputs: Default::default(),
            }),
            origin,
            cx,
        )
    });
    let Ok(ControlResult::FlowStarting {
        handle: RunHandle::Flow { run_id },
        ..
    }) = started
    else {
        panic!("the orchestrator's start was refused: {started:?}");
    };
    assert!(
        wait_for_run(cx, |cx| !run_is_held(cx, lane.path())).await,
        "the first run did not finish"
    );

    let run_dir = flow_paths::runs_dir(lane.path()).join(&run_id);
    let log = daruda_flow::run_log::log_file(&run_dir);
    let text = std::fs::read_to_string(&log).expect("the run log");
    let cut = text
        .lines()
        .position(|line| {
            line.contains("\"attempt_passed\"") && line.contains(&format!("\"{last}\""))
        })
        .expect("`last` passed");
    // Dated a minute back: a resume right after the last record waits out
    // the engine's settle window for the dead run's orphans.
    let kept: String = text
        .lines()
        .take(cut + 1)
        .map(|line| {
            let mut record: serde_json::Value = serde_json::from_str(line).expect("a record");
            let at_ms = record["at_ms"].as_u64().expect("a record's time");
            record["at_ms"] = at_ms.saturating_sub(60_000).into();
            format!("{record}\n")
        })
        .collect();
    std::fs::write(&log, kept).expect("cut the log");
    std::fs::remove_file(run_dir.join("DONE")).expect("the end's marker");
    write_lock(&ws, lane.path(), 0, &run_id, cx);
    Interrupted {
        lane,
        ws,
        run_id,
        run_dir,
        orchestrator,
    }
}

fn write_lock(
    ws: &gpui::Entity<Workspace>,
    lane: &std::path::Path,
    pid: u32,
    run_id: &str,
    cx: &mut TestAppContext,
) {
    let lock_dir = ws.read_with(cx, |ws, _| {
        flow_paths::lane_lock_dir(&ws.lock_root, lane).expect("the lane resolves")
    });
    std::fs::create_dir_all(&lock_dir).expect("lock dir");
    std::fs::write(
        lock_dir.join(".lock"),
        format!("pid: {pid}\nrun_id: {run_id}\nstarted_unix_secs: 1\n"),
    )
    .expect("lock");
}

fn status(run: &Interrupted, cx: &mut TestAppContext) -> daruda_flow::marker::RunStatus {
    let lock_dir = run.ws.read_with(cx, |ws, _| {
        flow_paths::lane_lock_dir(&ws.lock_root, run.lane.path()).expect("the lane resolves")
    });
    daruda_flow::marker::run_status(
        &run.run_dir,
        Some(&lock_dir),
        &daruda_core::process::is_alive,
    )
}

fn deferred(dispatch: Dispatch) -> smol::channel::Receiver<Option<ControlOutcome>> {
    match dispatch {
        Dispatch::Deferred { outcome, .. } => outcome,
        Dispatch::Ready(outcome) => panic!("answered without asking: {outcome:?}"),
    }
}

fn refused_before_the_card(command: GatedCommand, cx: &mut TestAppContext) -> ControlError {
    cx.update(|cx| {
        let dispatch = exec::run_gated(command, CommandOrigin::Orchestrator, cx);
        assert_eq!(
            approval::waiting_count_for_test(cx),
            0,
            "a refusal no answer can change is not asked about"
        );
        match dispatch {
            Dispatch::Ready(Err(error)) => error,
            Dispatch::Ready(Ok(result)) => panic!("not refused: {result:?}"),
            Dispatch::Deferred { .. } => panic!("asked before refusing"),
        }
    })
}

/// The approved resume runs only what had not passed, keeps the handle and
/// who first started the run, and delivers its end to the orchestrator with
/// the solicitation captured before the card — after the phone turn closed.
#[gpui::test]
async fn an_approved_resume_runs_only_the_remaining_steps_under_the_same_handle(
    cx: &mut TestAppContext,
) {
    let run = interrupted_orchestrator_run(cx).await;
    assert_eq!((run.lines("first.log"), run.lines("second.log")), (1, 1));
    run.orchestrator.update(cx, |view, _| {
        view.start_phone_turn_for_test(std::time::Instant::now())
    });
    let outcome =
        deferred(cx.update(|cx| exec::run_gated(run.resume(), CommandOrigin::Orchestrator, cx)));
    cx.run_until_parked();
    let asked = cx.update(|cx| approval::waiting_summaries_for_test(cx));
    assert_eq!(
        asked,
        vec![crate::surface::strings::control::approval_flow_resume(
            &run.run_id,
            1,
            2
        )],
        "the card names the run and how far it got"
    );
    run.orchestrator.update(cx, |view, _| view.end_phone_turn());
    cx.update(|cx| resolve_only_pending_for_test(ApprovalChoice::Approved, cx));
    cx.run_until_parked();
    let answered = outcome.recv().await.expect("answered");
    let Some(Ok(ControlResult::FlowResuming { handle, name, lane })) = answered else {
        panic!("not resumed: {answered:?}");
    };
    assert_eq!(handle, run.handle(), "the handle the orchestrator holds");
    assert_eq!(name, run.run_id);
    let active = run
        .ws
        .read_with(cx, |ws, _| LaneHandle::new(ws.uuid(), ws.active));
    assert_eq!(lane, active);

    assert!(
        wait_for_run(cx, |cx| {
            run.run_dir.join("DONE").exists() && !run_is_held(cx, run.lane.path())
        })
        .await,
        "the resumed run did not finish"
    );
    assert_eq!(
        (run.lines("first.log"), run.lines("second.log")),
        (1, 2),
        "only the step that had not passed ran again"
    );
    assert_eq!(
        daruda_flow::run_log::reader::read(&run.run_dir).started_by,
        Some(StartedBy::Orchestrator),
        "who started the run is kept"
    );
    let key = RunKey::Flow(run.run_id.clone());
    cx.update(|cx| {
        assert!(RunsHost::run_scope(cx).contains(&key));
        let ends: Vec<_> = RunsHost::unreported_run_events(cx)
            .into_iter()
            .filter(|event| event.snapshot.key == key && event.solicited)
            .collect();
        assert_eq!(ends.len(), 1, "one solicited end for the resumed episode");
        assert!(matches!(
            ends[0].snapshot.state,
            RunState::Ended {
                ended_as: EndedAs::Completed,
                ..
            }
        ));
    });
}

#[gpui::test]
async fn a_refused_resume_leaves_the_run_where_it_stopped(cx: &mut TestAppContext) {
    let run = interrupted_orchestrator_run(cx).await;
    let outcome =
        deferred(cx.update(|cx| exec::run_gated(run.resume(), CommandOrigin::Orchestrator, cx)));
    cx.run_until_parked();
    cx.update(|cx| resolve_only_pending_for_test(ApprovalChoice::Refused, cx));
    cx.run_until_parked();
    assert_eq!(
        outcome.recv().await.expect("answered"),
        Some(Err(ControlError::ApprovalRefused))
    );
    assert!(!run_is_held(cx, run.lane.path()), "nothing was resumed");
    assert_eq!(status(&run, cx), daruda_flow::marker::RunStatus::Crashed);
    assert_eq!(run.lines("second.log"), 1);
}

#[gpui::test]
async fn an_unanswered_resume_times_out_and_resumes_nothing(cx: &mut TestAppContext) {
    let run = interrupted_orchestrator_run(cx).await;
    let outcome =
        deferred(cx.update(|cx| exec::run_gated(run.resume(), CommandOrigin::Orchestrator, cx)));
    cx.run_until_parked();
    // Past the card's own deadline and any extension a phone card earns.
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(300 + 120 + 1));
    cx.run_until_parked();
    assert_eq!(
        outcome.recv().await.expect("answered"),
        Some(Err(ControlError::ApprovalTimedOut))
    );
    assert!(!run_is_held(cx, run.lane.path()), "nothing was resumed");
    assert_eq!(status(&run, cx), daruda_flow::marker::RunStatus::Crashed);
}

#[gpui::test]
async fn a_handle_no_open_worktree_holds_is_refused_before_the_card(cx: &mut TestAppContext) {
    let (_lane, _ws, _flow, _wh) = workspace_with_a_flow(cx, TWO_STEPS);
    crate::test_support::register_test_orchestrator(cx);
    let error = refused_before_the_card(
        GatedCommand::FlowResume {
            run: RunHandle::Flow {
                run_id: "never-was".into(),
            },
        },
        cx,
    );
    assert_eq!(error, ControlError::FlowRunNotFound);
}

#[gpui::test]
async fn a_run_that_ended_is_refused_before_the_card(cx: &mut TestAppContext) {
    let run = interrupted_orchestrator_run(cx).await;
    std::fs::write(run.run_dir.join("FAILED"), "").expect("an ending");
    let error = refused_before_the_card(run.resume(), cx);
    assert_eq!(
        error,
        ControlError::FlowRunNotResumable {
            name: run.run_id.clone(),
            reason: NotResumableReason::Ended,
        }
    );
}

#[gpui::test]
async fn a_run_that_never_started_a_step_is_refused_before_the_card(cx: &mut TestAppContext) {
    let (lane, ws, _flow, _wh) = workspace_with_a_flow(cx, TWO_STEPS);
    crate::test_support::register_test_orchestrator(cx);
    let run_dir = flow_paths::runs_dir(lane.path()).join("empty-run");
    std::fs::create_dir_all(&run_dir).expect("run dir");
    write_lock(&ws, lane.path(), 0, "empty-run", cx);
    let error = refused_before_the_card(
        GatedCommand::FlowResume {
            run: RunHandle::Flow {
                run_id: "empty-run".into(),
            },
        },
        cx,
    );
    assert_eq!(
        error,
        ControlError::FlowRunNotResumable {
            name: "empty-run".into(),
            reason: NotResumableReason::NeverStarted,
        }
    );
}

/// Another live process holds the worktree: named like a start names it,
/// so the caller can say which run is in the way.
#[gpui::test]
async fn a_worktree_another_process_holds_is_refused_naming_the_holder(cx: &mut TestAppContext) {
    let run = interrupted_orchestrator_run(cx).await;
    write_lock(
        &run.ws,
        run.lane.path(),
        std::process::id(),
        "elsewhere",
        cx,
    );
    let error = refused_before_the_card(run.resume(), cx);
    assert_eq!(
        error,
        ControlError::FlowLocked {
            name: run.run_id.clone(),
            held_by: Some(RunHandle::Flow {
                run_id: "elsewhere".into()
            }),
        }
    );
}

/// The desktop's Continue picks the run up as the desktop's own episode,
/// still recorded as the orchestrator's run; and a resume asked while it
/// runs is refused as running.
#[gpui::test]
async fn the_flows_panel_resume_is_the_desktops_episode(cx: &mut TestAppContext) {
    let run = interrupted_orchestrator_run(cx).await;
    std::fs::remove_file(run.lane.path().join("go")).expect("hold `second` open");
    let here = run.ws.read_with(cx, |ws, _| ws.active);
    run.ws
        .update(cx, |ws, cx| ws.resume_flow_run(here, &run.run_dir, cx));
    assert!(
        wait_for_run(cx, |cx| {
            cx.update(|cx| {
                RunsHost::flow_run(run.lane.path(), cx).is_some_and(|flow| {
                    matches!(
                        flow.view().node_state("second"),
                        crate::runs::NodeRunState::Running { .. }
                    )
                })
            })
        })
        .await,
        "the resumed run never started `second`"
    );
    let submitter = cx.update(|cx| {
        RunsHost::flow_run(run.lane.path(), cx)
            .map(|flow| flow.submitter_for_test())
            .expect("held")
    });
    assert_eq!(submitter, (StartedBy::Desktop, false));
    assert_eq!(
        refused_before_the_card(run.resume(), cx),
        ControlError::FlowRunNotResumable {
            name: run.run_id.clone(),
            reason: NotResumableReason::Running,
        }
    );

    run.ws.update(cx, |ws, cx| ws.stop_flow_run_in(here, cx));
    assert!(
        wait_for_run(cx, |cx| !run_is_held(cx, run.lane.path())).await,
        "the resumed run did not stop"
    );
    assert_eq!(
        daruda_flow::run_log::reader::read(&run.run_dir).started_by,
        Some(StartedBy::Orchestrator)
    );
}

/// A new process knows the run only from the run ledger: its owner crashed
/// mid-run and left `Owned` with the run's location. The restart judges it
/// interrupted, and the orchestrator resumes it under the same handle.
#[gpui::test]
async fn a_run_owned_before_a_restart_is_resumed_through_the_ledger(cx: &mut TestAppContext) {
    use daruda_store::run_ledger::{RunLedgerEvent, RunLedgerStore, RunLocation};
    let run = interrupted_orchestrator_run(cx).await;
    let (tasks, ledger) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let lock_root = run.ws.read_with(cx, |ws, _| ws.lock_root.clone());
    {
        let (store, _) = RunLedgerStore::in_directory(ledger.path(), "test").unwrap();
        store.record(RunLedgerEvent::Owned {
            key: daruda_store::run_ledger::LedgerKey::Flow {
                run_id: run.run_id.clone(),
            },
            solicited: true,
            location: Some(RunLocation {
                run_dir: run.run_dir.clone(),
                lock_dir: flow_paths::lane_lock_dir(&lock_root, run.lane.path()),
            }),
            at_ms: None,
        });
        store.flush(std::time::Duration::from_secs(5)).unwrap();
        assert!(store.take_answered().failure.is_none());
    }
    // The new process's host has no window effects: no open worktree is
    // asked where the run is, so only the ledger can say.
    cx.update(|cx| {
        drop(cx.remove_global::<RunsHost>());
        drop(cx.remove_global::<crate::runs::GlobalTasks>());
        let opened = RunsHost::open(daruda_store::tasks::TaskStore::in_directory(
            tasks.path(),
            None,
            "test",
            crate::runs::no_hook(),
        ))
        .with_run_ledger(RunLedgerStore::in_directory(ledger.path(), "test"))
        .with_flow_lock_root(lock_root.clone());
        RunsHost::install(opened, cx);
    });
    let key = RunKey::Flow(run.run_id.clone());
    assert!(
        wait_for_run(cx, |cx| {
            cx.update(|cx| {
                RunsHost::unreported_run_events(cx)
                    .iter()
                    .any(|event| event.snapshot.key == key)
            })
        })
        .await,
        "the restart never judged the run"
    );
    cx.update(|cx| {
        let pending = RunsHost::unreported_run_events(cx);
        assert_eq!(pending.len(), 1, "{pending:?}");
        assert!(matches!(
            pending[0].snapshot.state,
            RunState::Ended {
                ended_as: EndedAs::Crashed,
                ..
            }
        ));
        assert!(pending[0].solicited);
    });

    run.orchestrator.update(cx, |view, _| {
        view.start_phone_turn_for_test(std::time::Instant::now())
    });
    let outcome =
        deferred(cx.update(|cx| exec::run_gated(run.resume(), CommandOrigin::Orchestrator, cx)));
    cx.run_until_parked();
    run.orchestrator.update(cx, |view, _| view.end_phone_turn());
    cx.update(|cx| resolve_only_pending_for_test(ApprovalChoice::Approved, cx));
    cx.run_until_parked();
    let answered = outcome.recv().await.expect("answered");
    let Some(Ok(ControlResult::FlowResuming { handle, .. })) = answered else {
        panic!("not resumed: {answered:?}");
    };
    assert_eq!(handle, run.handle(), "the handle the orchestrator holds");
    assert!(
        wait_for_run(cx, |cx| {
            run.run_dir.join("DONE").exists() && !run_is_held(cx, run.lane.path())
        })
        .await,
        "the resumed run did not finish"
    );
    assert_eq!(
        (run.lines("first.log"), run.lines("second.log")),
        (1, 2),
        "only the step that had not passed ran again"
    );
    cx.update(|cx| {
        assert!(RunsHost::run_scope(cx).contains(&key));
        let completed: Vec<_> = RunsHost::unreported_run_events(cx)
            .into_iter()
            .filter(|event| {
                event.snapshot.key == key
                    && matches!(
                        event.snapshot.state,
                        RunState::Ended {
                            ended_as: EndedAs::Completed,
                            ..
                        }
                    )
            })
            .collect();
        assert_eq!(completed.len(), 1, "the resumed episode's end");
        assert!(completed[0].solicited, "asked from the phone");
    });
}

/// A live process that holds the worktree for this very run is the run
/// going on, not another run in its way.
#[gpui::test]
async fn a_live_holder_of_the_same_run_is_refused_as_running(cx: &mut TestAppContext) {
    let run = interrupted_orchestrator_run(cx).await;
    write_lock(
        &run.ws,
        run.lane.path(),
        std::process::id(),
        &run.run_id,
        cx,
    );
    assert_eq!(
        refused_before_the_card(run.resume(), cx),
        ControlError::FlowRunNotResumable {
            name: run.run_id.clone(),
            reason: NotResumableReason::Running,
        }
    );
}

/// A crash after the last step passed, before the run recorded its end:
/// the card says nothing runs again rather than that a step starts over.
#[gpui::test]
async fn a_run_cut_off_after_its_last_step_says_nothing_runs_again(cx: &mut TestAppContext) {
    let run = interrupted_run(CommandOrigin::Orchestrator, "second", cx).await;
    let outcome =
        deferred(cx.update(|cx| exec::run_gated(run.resume(), CommandOrigin::Orchestrator, cx)));
    cx.run_until_parked();
    assert_eq!(
        cx.update(|cx| approval::waiting_summaries_for_test(cx)),
        vec![crate::surface::strings::control::approval_flow_resume_all_passed(&run.run_id, 2)]
    );
    cx.update(|cx| resolve_only_pending_for_test(ApprovalChoice::Refused, cx));
    cx.run_until_parked();
    assert_eq!(
        outcome.recv().await.expect("answered"),
        Some(Err(ControlError::ApprovalRefused))
    );
}

/// The desktop's Continue lands while the orchestrator's card waits: the
/// check after approval finds the run going and refuses, so it is resumed
/// once, by the desktop.
#[gpui::test]
async fn a_desktop_resume_while_the_card_waits_is_not_resumed_twice(cx: &mut TestAppContext) {
    let run = interrupted_orchestrator_run(cx).await;
    std::fs::remove_file(run.lane.path().join("go")).expect("hold `second` open");
    let outcome =
        deferred(cx.update(|cx| exec::run_gated(run.resume(), CommandOrigin::Orchestrator, cx)));
    cx.run_until_parked();
    let here = run.ws.read_with(cx, |ws, _| ws.active);
    run.ws
        .update(cx, |ws, cx| ws.resume_flow_run(here, &run.run_dir, cx));
    assert!(
        wait_for_run(cx, |cx| {
            cx.update(|cx| {
                RunsHost::flow_run(run.lane.path(), cx).is_some_and(|flow| {
                    matches!(
                        flow.view().node_state("second"),
                        crate::runs::NodeRunState::Running { .. }
                    )
                })
            })
        })
        .await,
        "the desktop's resume never started `second`"
    );
    cx.update(|cx| resolve_only_pending_for_test(ApprovalChoice::Approved, cx));
    cx.run_until_parked();
    assert_eq!(
        outcome.recv().await.expect("answered"),
        Some(Err(ControlError::FlowRunNotResumable {
            name: run.run_id.clone(),
            reason: NotResumableReason::Running,
        }))
    );
    let submitter = cx.update(|cx| {
        RunsHost::flow_run(run.lane.path(), cx)
            .map(|flow| flow.submitter_for_test())
            .expect("held")
    });
    assert_eq!(
        submitter,
        (StartedBy::Desktop, false),
        "still the desktop's"
    );

    run.ws.update(cx, |ws, cx| ws.stop_flow_run_in(here, cx));
    assert!(
        wait_for_run(cx, |cx| !run_is_held(cx, run.lane.path())).await,
        "the resumed run did not stop"
    );
    assert_eq!(run.lines("first.log"), 1, "`first` never ran again");
}

/// A crashed run a running Task started from the phone: only the Task the
/// orchestrator holds is in its scope.
async fn interrupted_task_run(
    cx: &mut TestAppContext,
) -> (Interrupted, daruda_store::tasks::ExecutionRef) {
    use crate::runs::seed::{self, SeedEnd, SeedRun};
    let run = interrupted_run(CommandOrigin::Phone, "first", cx).await;
    let exec = run.ws.update(cx, |ws, cx| {
        let seeded = SeedRun {
            source: daruda_store::tasks::TaskExecutionSource::Flow {
                run_id: run.run_id.clone(),
                flow: "ship.yaml".into(),
            },
            ..SeedRun::chat(run.lane.path())
        };
        let task = seed::task(
            seed::spec(ws.projects[0].uuid, "Ship"),
            Some((&seeded, SeedEnd::Running)),
            cx,
        );
        let exec = seeded.exec(&task);
        RunsHost::register_run(RunKey::Task(exec.clone()), cx);
        exec
    });
    cx.update(|cx| {
        assert!(!RunsHost::run_scope(cx).contains(&RunKey::Flow(run.run_id.clone())));
    });
    (run, exec)
}

fn task_handle(exec: &daruda_store::tasks::ExecutionRef) -> RunHandle {
    RunHandle::Task {
        task_id: exec.task_id.clone(),
        execution_id: exec.execution_id.clone(),
    }
}

/// Approve the only card and answer what the resume said.
async fn approved(
    outcome: smol::channel::Receiver<Option<ControlOutcome>>,
    cx: &mut TestAppContext,
) -> ControlResult {
    cx.run_until_parked();
    cx.update(|cx| resolve_only_pending_for_test(ApprovalChoice::Approved, cx));
    cx.run_until_parked();
    match outcome.recv().await.expect("answered") {
        Some(Ok(result)) => result,
        other => panic!("not resumed: {other:?}"),
    }
}

/// The Task's handle, the one its result carried, resumes the Task's run;
/// the Task stays the run's only handle, so its end is told once.
#[gpui::test]
async fn a_tasks_handle_resumes_its_flow_under_the_tasks_handle(cx: &mut TestAppContext) {
    let (run, exec) = interrupted_task_run(cx).await;
    let command = GatedCommand::FlowResume {
        run: task_handle(&exec),
    };
    let outcome =
        deferred(cx.update(|cx| exec::run_gated(command, CommandOrigin::Orchestrator, cx)));
    let ControlResult::FlowResuming { handle, name, .. } = approved(outcome, cx).await else {
        panic!("not a resume");
    };
    assert_eq!(handle, task_handle(&exec));
    assert_eq!(name, run.run_id);
    assert!(
        wait_for_run(cx, |cx| {
            run.run_dir.join("DONE").exists() && !run_is_held(cx, run.lane.path())
        })
        .await,
        "the resumed run did not finish"
    );
    assert_eq!((run.lines("first.log"), run.lines("second.log")), (1, 2));
    let task = RunKey::Task(exec);
    let flow = RunKey::Flow(run.run_id.clone());
    assert!(
        wait_for_run(cx, |cx| {
            cx.update(|cx| {
                RunsHost::unreported_run_events(cx).iter().any(|event| {
                    event.snapshot.key == task
                        && matches!(
                            event.snapshot.state,
                            RunState::Ended {
                                ended_as: EndedAs::Completed,
                                ..
                            }
                        )
                })
            })
        })
        .await,
        "the Task's own end never came"
    );
    cx.update(|cx| {
        let scope = RunsHost::run_scope(cx);
        assert!(scope.contains(&task));
        assert!(!scope.contains(&flow), "one handle per run");
        assert!(
            RunsHost::unreported_run_events(cx)
                .iter()
                .all(|event| event.snapshot.key != flow),
            "the flow's end is not told beside the Task's"
        );
    });
}

/// The flow's own handle, for a run a Task the orchestrator holds runs as,
/// resumes it as that Task's: no second key joins the scope.
#[gpui::test]
async fn a_tasks_flow_handle_resumes_it_as_the_tasks(cx: &mut TestAppContext) {
    let (run, exec) = interrupted_task_run(cx).await;
    let outcome =
        deferred(cx.update(|cx| exec::run_gated(run.resume(), CommandOrigin::Orchestrator, cx)));
    let ControlResult::FlowResuming { handle, .. } = approved(outcome, cx).await else {
        panic!("not a resume");
    };
    assert_eq!(
        handle,
        task_handle(&exec),
        "the handle the orchestrator holds"
    );
    assert!(
        wait_for_run(cx, |cx| {
            run.run_dir.join("DONE").exists() && !run_is_held(cx, run.lane.path())
        })
        .await,
        "the resumed run did not finish"
    );
    cx.update(|cx| {
        assert!(!RunsHost::run_scope(cx).contains(&RunKey::Flow(run.run_id.clone())));
    });
}
