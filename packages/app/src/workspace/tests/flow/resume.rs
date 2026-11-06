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
        GatedCommand::FlowResume {
            run_id: self.run_id.clone(),
        }
    }
}

/// Start [`TWO_STEPS`] as the orchestrator, let it finish, then rewrite what
/// it left into what a crash after `first` leaves: a log ending there, no
/// marker, and a lock naming a pid that is gone (0 never lives).
async fn interrupted_orchestrator_run(cx: &mut TestAppContext) -> Interrupted {
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
            CommandOrigin::Orchestrator,
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
        .position(|line| line.contains("\"attempt_passed\"") && line.contains("\"first\""))
        .expect("`first` passed");
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
            run_id: "never-was".into(),
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
            run_id: "empty-run".into(),
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
