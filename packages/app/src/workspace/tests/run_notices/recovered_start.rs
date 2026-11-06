//! A restart that owes the phone a result brings the orchestrator up once the
//! first window can host it; results the desktop asked for wait for a start.
use super::*;
use crate::runs::seed::{self, SeedEnd, SeedRun};
use crate::runs::watch::Origin;
use daruda_store::run_ledger::RunLedgerStore;
use daruda_store::tasks::TaskStore;
use futures::{FutureExt as _, StreamExt as _};
use gpui::BorrowAppContext as _;

/// The task log and run ledger one process opens, kept for the next.
struct Profile {
    tasks: tempfile::TempDir,
    ledger: tempfile::TempDir,
}

impl Profile {
    fn new() -> Self {
        Self {
            tasks: tempfile::tempdir().unwrap(),
            ledger: tempfile::tempdir().unwrap(),
        }
    }

    fn start(&self, cx: &mut gpui::App) {
        let opened = RunsHost::open(TaskStore::in_directory(
            self.tasks.path(),
            None,
            "test",
            crate::runs::no_hook(),
        ))
        .with_run_ledger(RunLedgerStore::in_directory(self.ledger.path(), "test"));
        RunsHost::install(opened, cx);
    }

    /// An orchestrator Task ended unreported, then daruda quit.
    fn owe_and_quit(&self, solicited: bool, cx: &mut TestAppContext) -> RunKey {
        cx.update(|cx| {
            self.start(cx);
            let run = SeedRun::chat("/wt");
            let task = seed::task(
                seed::spec(Default::default(), "summarize the release"),
                Some((&run, SeedEnd::Done)),
                cx,
            );
            let key = RunKey::Task(run.exec(&task));
            RunsHost::register_run_solicited(key.clone(), solicited, cx);
            RunsHost::quit_flush(
                std::time::Instant::now() + std::time::Duration::from_secs(5),
                cx,
            );
            drop(cx.remove_global::<RunsHost>());
            drop(cx.remove_global::<crate::runs::GlobalTasks>());
            key
        })
    }

    /// A phone-asked orchestrator Task was still running when daruda quit;
    /// the next start's reconciliation ends it as a recovered, solicited end.
    fn restart_with_carried_task(&self, cx: &mut TestAppContext) {
        cx.update(|cx| {
            self.start(cx);
            let spec = seed::spec(Default::default(), "summarize the release");
            let id = spec.id.clone();
            let run = SeedRun::chat("/wt");
            let mut events = seed::created(spec);
            events.extend(seed::run(&id, &run, SeedEnd::Running));
            for event in &mut events {
                if let daruda_store::tasks::TaskEvent::DispatchRequested { started_by, .. } = event
                {
                    *started_by = Some(daruda_store::tasks::machine::StartedBy::Orchestrator);
                }
            }
            seed::commit_now(events, cx);
            RunsHost::register_run_solicited(RunKey::Task(run.exec(&id)), true, cx);
            RunsHost::quit_flush(
                std::time::Instant::now() + std::time::Duration::from_secs(5),
                cx,
            );
            drop(cx.remove_global::<RunsHost>());
            drop(cx.remove_global::<crate::runs::GlobalTasks>());
            self.start(cx);
            let pending = RunsHost::unreported_run_events(cx);
            assert_eq!(pending.len(), 1);
            assert_eq!(pending[0].origin, Origin::Recovered);
            assert!(pending[0].solicited);
        });
    }

    /// [`Self::owe_and_quit`], then start again with no window open yet.
    fn restart_owing(&self, solicited: bool, cx: &mut TestAppContext) -> RunKey {
        let key = self.owe_and_quit(solicited, cx);
        cx.update(|cx| {
            self.start(cx);
            let pending = RunsHost::unreported_run_events(cx);
            assert_eq!(pending.len(), 1);
            assert_eq!(pending[0].origin, Origin::Recovered);
            assert_eq!(pending[0].solicited, solicited);
        });
        key
    }
}

/// Remote channels, presence and fake agent connections, with the
/// orchestrator switched `enabled`.
fn surroundings(
    enabled: bool,
    cx: &mut TestAppContext,
) -> (
    futures::channel::mpsc::UnboundedReceiver<crate::remote_channel::global::Pending>,
    std::rc::Rc<std::cell::RefCell<Vec<daruda_acp::HandleProbe>>>,
) {
    crate::test_support::init_gpui_component(cx);
    cx.update(|cx| {
        crate::app_presence::init(cx);
        crate::app_presence::seed_for_test(
            crate::platform::presence::AwaySignal::HERE,
            true,
            Some(std::time::Duration::ZERO),
            cx,
        );
        let outbound = crate::remote_channel::global::tests::setup(cx);
        cx.update_global::<crate::settings_store::SettingsStore, _>(|store, _| {
            let mut config = (*store.user()).clone();
            config.orchestrator.enabled = enabled;
            store.set_user_for_testing(config);
        });
        crate::orchestrator::seed_control_surface_for_test(cx);
        let probes = crate::workspace::Workspace::fake_agent_connections_for_test(cx);
        (outbound, probes)
    })
}

fn orchestrator_pane(cx: &mut TestAppContext) -> Option<crate::telegram::bridge::PaneRef> {
    cx.update(|cx| crate::orchestrator::pane(cx))
}

#[gpui::test]
fn a_restart_owing_the_phone_starts_the_orchestrator_and_answers_once(cx: &mut TestAppContext) {
    let profile = Profile::new();
    let key = profile.restart_owing(true, cx);
    let (mut outbound, probes) = surroundings(true, cx);
    cx.run_until_parked();
    assert!(
        orchestrator_pane(cx).is_none(),
        "no window, nothing to host"
    );

    let _window = crate::test_support::workspace_for_control(cx);
    cx.run_until_parked();
    let pane = orchestrator_pane(cx).expect("the first window hosts the orchestrator");
    let prompts = sent(cx);
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].contains("summarize the release"));
    assert!(prompts[0].contains("requested before daruda restarted"));
    assert_eq!(probes.borrow_mut().len(), 1);
    assert_eq!(probes.borrow_mut()[0].drain().0, 1);
    super::solicited::answer_reaches_phone_once(pane, &mut outbound, true, false, cx);
    cx.update(|cx| {
        assert!(
            RunsHost::unreported_run_events(cx)
                .iter()
                .any(|event| event.snapshot.key == key),
            "a notice is not a receipt: daruda_run_wait still reports it"
        );
    });
}

#[gpui::test]
fn a_result_the_desktop_asked_for_waits_for_the_next_start(cx: &mut TestAppContext) {
    let profile = Profile::new();
    profile.restart_owing(false, cx);
    let (_outbound, _probes) = surroundings(true, cx);
    let _window = crate::test_support::workspace_for_control(cx);
    cx.run_until_parked();
    assert!(orchestrator_pane(cx).is_none());

    cx.update(crate::orchestrator::ensure)
        .expect("started on request");
    cx.run_until_parked();
    let prompts = sent(cx);
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].contains("requested before daruda restarted"));
    let pane = orchestrator_pane(cx).unwrap();
    host(cx).update(cx, |ws, cx| {
        assert!(
            ws.agent_chat_view(pane.pane)
                .unwrap()
                .read(cx)
                .phone_turn()
                .is_none(),
            "a desktop result arms no phone turn"
        );
    });
}

#[gpui::test]
fn a_disabled_orchestrator_is_not_started_and_the_result_stays_pending(cx: &mut TestAppContext) {
    let profile = Profile::new();
    let key = profile.restart_owing(true, cx);
    let (mut outbound, _probes) = surroundings(false, cx);
    let _window = crate::test_support::workspace_for_control(cx);
    cx.run_until_parked();
    assert!(orchestrator_pane(cx).is_none());
    assert!(outbound.next().now_or_never().is_none());
    cx.update(|cx| {
        let pending = RunsHost::unreported_run_events(cx);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].snapshot.key, key);
    });
}

/// A start that fails is toasted in the window that would have hosted it,
/// once per process, and the phone's results stay unreported.
#[gpui::test]
fn a_failed_start_is_toasted_once_and_the_result_stays_pending(cx: &mut TestAppContext) {
    let profile = Profile::new();
    let key = profile.restart_owing(true, cx);
    let (_outbound, probes) = surroundings(true, cx);
    cx.update(|cx| {
        cx.update_global::<crate::settings_store::SettingsStore, _>(|store, _| {
            let mut config = (*store.user()).clone();
            config.orchestrator.agent_id = Some("no-such-agent".into());
            store.set_user_for_testing(config);
        });
    });
    let first = crate::test_support::workspace_for_control(cx);
    let _second = crate::test_support::workspace_for_control(cx);
    cx.run_until_parked();
    assert!(orchestrator_pane(cx).is_none());
    assert!(probes.borrow().is_empty());
    let toasts: Vec<_> = first.workspace.read_with(cx, |ws, _| {
        ws.error_history()
            .iter()
            .filter(|report| report.dedup_key.as_deref() == Some("orchestrator.recovered_start"))
            .map(|report| report.context.get("reason").cloned())
            .collect()
    });
    assert_eq!(toasts, vec![Some("Unresolvable".to_owned())]);
    cx.update(|cx| {
        let pending = RunsHost::unreported_run_events(cx);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].snapshot.key, key);
    });
}

#[gpui::test]
fn more_windows_and_a_closed_orchestrator_never_start_it_twice(cx: &mut TestAppContext) {
    let profile = Profile::new();
    profile.restart_owing(true, cx);
    let (_outbound, probes) = surroundings(true, cx);
    let first = crate::test_support::workspace_for_control(cx);
    let _second = crate::test_support::workspace_for_control(cx);
    cx.run_until_parked();
    let pane = orchestrator_pane(cx).expect("started once");
    assert_eq!(
        pane.workspace,
        first.workspace.read_with(cx, |ws, _| ws.uuid()),
        "hosted where the start found a window"
    );
    assert_eq!(probes.borrow().len(), 1);

    cx.update(WindowRegistry::clear_orchestrator);
    let _third = crate::test_support::workspace_for_control(cx);
    cx.run_until_parked();
    assert!(
        orchestrator_pane(cx).is_none(),
        "the decision is made once per process"
    );
    assert_eq!(probes.borrow().len(), 1);
}

/// A restored flow is judged off the UI thread, so its recovered end can
/// arrive after the first window registered.
#[gpui::test]
fn a_recovered_end_announced_after_the_first_window_starts_it(cx: &mut TestAppContext) {
    let profile = Profile::new();
    profile.owe_and_quit(true, cx);
    let (_outbound, probes) = surroundings(true, cx);
    let _window = crate::test_support::workspace_for_control(cx);
    cx.run_until_parked();
    assert!(orchestrator_pane(cx).is_none(), "nothing was owed yet");
    // Installing over the window's host keeps its effects, so the ledger's
    // recovered episode is announced to the window like a judged end.
    cx.update(|cx| profile.start(cx));
    cx.run_until_parked();
    assert!(orchestrator_pane(cx).is_some());
    assert_eq!(sent(cx).len(), 1);
    assert_eq!(probes.borrow().len(), 1);
}

/// Startup reconciliation settles a carried Task before the first window; the
/// phone is still owed its answer, so that window starts the orchestrator.
#[gpui::test]
fn a_carried_phone_task_settled_at_startup_starts_it_and_answers_once(cx: &mut TestAppContext) {
    let profile = Profile::new();
    profile.restart_with_carried_task(cx);
    let (mut outbound, probes) = surroundings(true, cx);
    let _window = crate::test_support::workspace_for_control(cx);
    cx.run_until_parked();
    let pane = orchestrator_pane(cx).expect("the first window hosts the orchestrator");
    let prompts = sent(cx);
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].contains("summarize the release"));
    assert!(prompts[0].contains("requested before daruda restarted"));
    assert_eq!(probes.borrow_mut()[0].drain().0, 1);
    super::solicited::answer_reaches_phone_once(pane, &mut outbound, true, false, cx);
}

/// Past the first window, only a recovered end may start it: a live phone
/// result while the orchestrator is closed waits like any other.
#[gpui::test]
fn a_live_phone_result_after_the_first_window_does_not_start_it(cx: &mut TestAppContext) {
    let (_outbound, probes) = surroundings(true, cx);
    let _first = crate::test_support::workspace_for_control(cx);
    cx.run_until_parked();
    cx.update(|cx| {
        let snapshot = ended("live-phone");
        RunsHost::register_run_solicited(snapshot.key.clone(), true, cx);
        RunsHost::seed_run_wait(snapshot, cx);
    });
    cx.run_until_parked();
    let _second = crate::test_support::workspace_for_control(cx);
    cx.run_until_parked();
    assert!(orchestrator_pane(cx).is_none());
    assert!(probes.borrow().is_empty());
}
