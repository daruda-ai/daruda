//! What every edit of a held run tells the windows about its questions, and
//! what it leaves in the watch.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use daruda_flow::event::{FlowEvent, RunEnd};
use daruda_flow::runner::{AskOwner, AskRequest, PendingAsk};
use gpui::TestAppContext;

use crate::runs::flows::{AnswerRefused, FlowAskOutcome, FlowChange, FlowChanged, FlowSource};
use crate::runs::result::{RunKey, RunState, WaitingOn};
use crate::runs::{RunView, RunsHost};

/// Every change told to the windows, as a line each.
struct Told(Rc<RefCell<Vec<String>>>);

impl crate::runs::RunsEffects for Told {
    fn stop_execution(&self, _: &daruda_store::tasks::ExecutionRef, _: &mut gpui::App) {}

    fn flow_changed(&self, change: &FlowChange, cx: &mut gpui::App) {
        if let FlowChanged::AskClosed { ask_id, .. } = change.what {
            assert!(
                RunsHost::flow_run(change.worktree.as_path(), cx)
                    .is_none_or(|run| run.view().question(ask_id).is_none()),
                "question {ask_id} was told closed while still held"
            );
        }
        self.0.borrow_mut().push(line(&change.what));
    }
}

fn line(what: &FlowChanged) -> String {
    match what {
        FlowChanged::Repainted => "repainted".into(),
        FlowChanged::Progressed { .. } => "progressed".into(),
        FlowChanged::Asked { ask_id, alone, .. } => format!("asked {ask_id} alone={alone}"),
        FlowChanged::AskClosed {
            ask_id, outcome, ..
        } => match outcome {
            FlowAskOutcome::Answered(_) => format!("answered {ask_id}"),
            FlowAskOutcome::Withdrawn => format!("withdrawn {ask_id}"),
        },
        FlowChanged::Ended(_) => "ended".into(),
    }
}

struct Seeded {
    tree: PathBuf,
    run_dir: PathBuf,
    told: Rc<RefCell<Vec<String>>>,
}

impl Seeded {
    /// What was told since the last call.
    fn told(&self) -> Vec<String> {
        std::mem::take(&mut *self.told.borrow_mut())
    }
}

fn seeded(name: &str, cx: &mut TestAppContext) -> Seeded {
    let tree = PathBuf::from("/work").join(name);
    let run_dir = tree.join("run");
    let told = Rc::new(RefCell::new(Vec::new()));
    cx.update(|cx| {
        RunsHost::ensure_installed(cx);
        RunsHost::install_effects(Rc::new(Told(told.clone())), cx);
        RunsHost::seed_flow_run(
            tree.clone(),
            RunView::new(
                run_dir.clone(),
                FlowSource::Resumed {
                    run_dir: run_dir.clone(),
                },
                vec![],
            ),
            cx,
        );
    });
    told.borrow_mut().clear();
    Seeded {
        tree,
        run_dir,
        told,
    }
}

fn ask(
    ask_id: u64,
    node: &str,
) -> (
    PendingAsk,
    smol::channel::Receiver<daruda_acp::PermissionDecision>,
) {
    let (reply, receiver) = smol::channel::bounded(1);
    let pending = PendingAsk {
        owner: AskOwner::Node {
            node: node.into(),
            generation: 0,
            attempt: 1,
        },
        ask_id,
        request: AskRequest {
            tool: "write".into(),
            detail: None,
            options: vec![],
        },
        reply,
    };
    (pending, receiver)
}

fn park(
    seeded: &Seeded,
    run_dir: &Path,
    ask_id: u64,
    node: &str,
    cx: &mut TestAppContext,
) -> smol::channel::Receiver<daruda_acp::PermissionDecision> {
    let (pending, receiver) = ask(ask_id, node);
    cx.update(|cx| RunsHost::park_flow_ask(&seeded.tree, run_dir, pending, cx));
    receiver
}

fn event(seeded: &Seeded, event: FlowEvent, cx: &mut TestAppContext) {
    cx.update(|cx| RunsHost::apply_flow_event_for_test(&seeded.tree, &event, cx));
}

/// The run's blocked episodes the watch still has to report.
fn blocks(cx: &mut TestAppContext) -> Vec<(u64, Option<WaitingOn>)> {
    cx.update(|cx| {
        RunsHost::unreported_run_events(cx)
            .into_iter()
            .filter(|event| event.snapshot.key == RunKey::Flow("run".into()))
            .filter_map(|event| match event.snapshot.state {
                RunState::Running { waiting_on, .. } => Some((event.sequence, waiting_on)),
                _ => None,
            })
            .collect()
    })
}

fn on(node: &str) -> Option<WaitingOn> {
    Some(WaitingOn::Question { node: node.into() })
}

#[gpui::test]
fn a_stop_withdraws_every_question_before_repainting(cx: &mut TestAppContext) {
    let run = seeded("stop", cx);
    let first = park(&run, &run.run_dir, 1, "a", cx);
    let second = park(&run, &run.run_dir, 2, "b", cx);
    assert_eq!(run.told(), ["asked 1 alone=true", "asked 2 alone=false"]);

    assert!(cx.update(|cx| RunsHost::stop_flow(&run.tree, cx)));
    assert_eq!(run.told(), ["withdrawn 1", "withdrawn 2", "repainted"]);
    assert!(first.is_closed() && second.is_closed());
}

#[gpui::test]
fn an_answer_closes_only_its_own_question(cx: &mut TestAppContext) {
    let run = seeded("answer", cx);
    let _first = park(&run, &run.run_dir, 1, "a", cx);
    let second = park(&run, &run.run_dir, 2, "b", cx);
    run.told();
    let answer = |ask_id, cx: &mut TestAppContext| {
        cx.update(|cx| {
            RunsHost::answer_flow_ask(
                &run.tree,
                &run.run_dir,
                ask_id,
                daruda_acp::PermissionDecision::Cancelled,
                cx,
            )
        })
    };

    assert_eq!(answer(1, cx), Ok(()));
    assert_eq!(run.told(), ["answered 1"]);
    assert_eq!(answer(1, cx), Err(AnswerRefused::AlreadyAnswered));
    assert_eq!(run.told(), Vec::<String>::new());
    drop(second);
    assert_eq!(answer(2, cx), Err(AnswerRefused::Closed));
    assert_eq!(run.told(), ["withdrawn 2"]);
    assert_eq!(answer(3, cx), Err(AnswerRefused::NotOpen));
    assert_eq!(run.told(), Vec::<String>::new());
}

/// A question the engine abandoned goes down, and is told so, before the
/// one that replaces it is told; the new one is then alone.
#[gpui::test]
fn a_new_question_withdraws_an_abandoned_one_first(cx: &mut TestAppContext) {
    let run = seeded("park", cx);
    drop(park(&run, &run.run_dir, 1, "a", cx));
    run.told();
    let _second = park(&run, &run.run_dir, 2, "b", cx);
    assert_eq!(run.told(), ["withdrawn 1", "asked 2 alone=true"]);
}

#[gpui::test]
fn an_event_withdraws_only_the_abandoned_questions(cx: &mut TestAppContext) {
    let run = seeded("event", cx);
    let _first = park(&run, &run.run_dir, 1, "a", cx);
    let second = park(&run, &run.run_dir, 2, "b", cx);
    let _third = park(&run, &run.run_dir, 3, "c", cx);
    drop(second);
    run.told();
    event(
        &run,
        FlowEvent::NodeStarted {
            node: "c".into(),
            attempt: 1,
        },
        cx,
    );
    assert_eq!(run.told(), ["withdrawn 2", "progressed"]);
    let held = cx.update(|cx| {
        let run = RunsHost::flow_run(&run.tree, cx).expect("held");
        run.view()
            .questions()
            .map(|ask| ask.ask_id())
            .collect::<Vec<_>>()
    });
    assert_eq!(held, [1, 3]);
}

#[gpui::test]
fn the_end_withdraws_every_question_before_it_is_told(cx: &mut TestAppContext) {
    let run = seeded("end", cx);
    let first = park(&run, &run.run_dir, 1, "a", cx);
    run.told();
    event(&run, FlowEvent::RunEnded { end: RunEnd::Done }, cx);
    assert_eq!(run.told(), ["withdrawn 1", "ended"]);
    assert!(first.is_closed());
    assert!(cx.update(|cx| RunsHost::flow_run(&run.tree, cx).is_none()));
}

/// The asks channel closing withdraws what is still up.
#[gpui::test]
fn closing_the_asks_withdraws_what_is_still_up(cx: &mut TestAppContext) {
    let run = seeded("asks-done", cx);
    let first = park(&run, &run.run_dir, 1, "a", cx);
    run.told();
    cx.update(|cx| RunsHost::withdraw_flow_asks(&run.tree, &run.run_dir, cx));
    assert_eq!(run.told(), ["withdrawn 1", "repainted"]);
    assert!(first.is_closed());
}

/// What an earlier run of the same worktree still sends finds nothing: its
/// question is released and the current run's are untouched.
#[gpui::test]
fn an_earlier_runs_messages_change_nothing(cx: &mut TestAppContext) {
    let run = seeded("earlier", cx);
    let current = park(&run, &run.run_dir, 1, "a", cx);
    run.told();
    let earlier = run.tree.join("earlier");
    let late = park(&run, &earlier, 2, "b", cx);
    assert!(late.is_closed(), "the earlier engine is released");
    cx.update(|cx| RunsHost::withdraw_flow_asks(&run.tree, &earlier, cx));
    let answered = cx.update(|cx| {
        RunsHost::answer_flow_ask(
            &run.tree,
            &earlier,
            1,
            daruda_acp::PermissionDecision::Cancelled,
            cx,
        )
    });
    assert_eq!(answered, Err(AnswerRefused::NotOpen));
    assert_eq!(run.told(), Vec::<String>::new());
    assert!(!current.is_closed());
}

/// A question asked right after an unanswered one was abandoned is a new
/// waiting episode, not the old one renamed; one asked beside an open one
/// continues it.
#[gpui::test]
fn a_question_after_an_abandoned_one_is_a_new_waiting_episode(cx: &mut TestAppContext) {
    let run = seeded("episode", cx);
    cx.update(|cx| RunsHost::register_run(RunKey::Flow("run".into()), cx));
    let first = park(&run, &run.run_dir, 1, "a", cx);
    let [(abandoned, ref waiting)] = blocks(cx)[..] else {
        panic!("one blocked episode");
    };
    assert_eq!(*waiting, on("a"));

    drop(first);
    let _second = park(&run, &run.run_dir, 2, "b", cx);
    let [(renewed, ref waiting)] = blocks(cx)[..] else {
        panic!("one blocked episode");
    };
    assert_ne!(renewed, abandoned);
    assert_eq!(*waiting, on("b"));

    let _third = park(&run, &run.run_dir, 3, "c", cx);
    assert_eq!(blocks(cx), [(renewed, on("b"))]);
}
