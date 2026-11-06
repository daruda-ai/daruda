//! Starting the orchestrator for results the phone was owed across a restart.
//!
//! Notices only reach a running orchestrator, and it starts lazily. A phone
//! request whose result survived a restart would otherwise wait until someone
//! happened to start one, so the first window that can host it starts it once.
//! Results the desktop asked for keep waiting for an ordinary start.

use daruda_store::observability::error_report::{ErrorReport, ErrorSeverity};
use daruda_store::observability::log_writer::LogWriter;
use gpui::{App, Global};

use super::EnsureError;
use crate::runs::{
    RunsHost,
    result::RunKey,
    watch::{Origin, RunEvent},
};
use crate::window_registry::WindowRegistry;

/// This process's progress toward its one start.
#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum AutoStart {
    /// No workspace has registered yet: every phone result is from before.
    #[default]
    AwaitingWindow,
    /// The first window owed nothing; only a later recovered end may start it,
    /// so a live result in ordinary operation never does.
    Watching,
    /// Started, or decided not to: windows opening again, or the orchestrator
    /// closing afterwards, change nothing.
    Decided,
}
impl Global for AutoStart {}

#[derive(Clone, Copy)]
enum Trigger {
    WindowRegistered,
    RecoveredEnd,
}

/// Called as a workspace registers, inside its construction.
pub(crate) fn on_window_registered(cx: &mut App) {
    schedule(Trigger::WindowRegistered, cx);
}

/// A restored run judged from disk ends off the UI thread, so its recovered
/// end can arrive after the first window registered.
pub(crate) fn on_run_event(event: &RunEvent, cx: &mut App) {
    if event.origin == Origin::Recovered && event.solicited {
        schedule(Trigger::RecoveredEnd, cx);
    }
}

/// Decide after the current update: seeding the host window from inside its
/// construction or a run event would re-enter it.
fn schedule(trigger: Trigger, cx: &mut App) {
    if *cx.default_global::<AutoStart>() != AutoStart::Decided {
        cx.defer(move |cx| decide(trigger, cx));
    }
}

fn decide(trigger: Trigger, cx: &mut App) {
    let state = *cx.default_global::<AutoStart>();
    if WindowRegistry::first_workspace(cx).is_none() {
        return;
    }
    let events = RunsHost::unreported_run_events(cx);
    let scope = RunsHost::run_scope(cx);
    let owed: Vec<_> = phone_owed(&events, &scope).collect();
    let starts = match (trigger, state) {
        (_, AutoStart::Decided) => return,
        (Trigger::WindowRegistered, AutoStart::AwaitingWindow) => !owed.is_empty(),
        (Trigger::WindowRegistered, AutoStart::Watching) => return,
        (Trigger::RecoveredEnd, _) => owed.iter().any(|e| e.origin == Origin::Recovered),
    };
    if !starts {
        // Only the first window's look settles what was owed from before.
        if matches!(trigger, Trigger::WindowRegistered) {
            cx.set_global(AutoStart::Watching);
        }
        return;
    }
    cx.set_global(AutoStart::Decided);
    start(owed.len(), cx);
}

/// Runs deferred, outside any window's update, so the failure toast can
/// update the host workspace directly.
fn start(owed: usize, cx: &mut App) {
    let outcome = if super::pane(cx).is_some() {
        "already running"
    } else {
        match super::ensure(cx) {
            Ok(_) => "started",
            Err(EnsureError::Disabled) => "disabled; results stay pending",
            Err(error) => {
                report_failure(owed, &error, cx);
                return;
            }
        }
    };
    log(ErrorSeverity::Info, owed, outcome);
}

/// Nobody clicked anything, so a log line alone would leave the phone's
/// results waiting without a word: the failure is toasted like a chip start,
/// in the window the start would have hosted the orchestrator in.
fn report_failure(owed: usize, error: &EnsureError, cx: &mut App) {
    let report = ErrorReport::new(crate::surface::strings::orchestrator::start_failed())
        .severity(ErrorSeverity::Warning)
        .with_context("results", owed.to_string())
        .with_context("reason", format!("{error:?}"))
        .at(file!(), line!())
        .dedup(DEDUP)
        .build();
    match super::existing_host(cx) {
        Some((_, host)) => super::report_start_failure(&host, report, cx),
        None => LogWriter::log(report),
    }
}

const DEDUP: &str = "orchestrator.recovered_start";

/// Unreported episodes in the orchestrator's scope that a phone asked for:
/// the ones whose notice turn answers the phone.
fn phone_owed<'a>(
    events: &'a [RunEvent],
    scope: &'a [RunKey],
) -> impl Iterator<Item = &'a RunEvent> {
    events
        .iter()
        .filter(|event| event.solicited && scope.contains(&event.snapshot.key))
}

fn log(severity: ErrorSeverity, owed: usize, outcome: &str) {
    LogWriter::log(
        ErrorReport::new("Orchestrator start for phone results owed across a restart")
            .severity(severity)
            .with_context("results", owed.to_string())
            .with_context("outcome", outcome)
            .at(file!(), line!())
            .dedup(DEDUP)
            .build(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runs::result::{EndedAs, RunSnapshot, RunState};

    fn event(id: &str, origin: Origin, solicited: bool) -> RunEvent {
        let mut event = RunEvent::for_test(
            1,
            RunSnapshot {
                key: RunKey::Flow(id.into()),
                what: id.into(),
                why: None,
                state: RunState::Ended {
                    ended_as: EndedAs::Completed,
                    text: None,
                    pointers: Default::default(),
                },
            },
        );
        event.origin = origin;
        event.solicited = solicited;
        event
    }

    #[test]
    fn phone_results_in_scope_are_owed_whatever_their_origin() {
        let events = [
            event("phone", Origin::Recovered, true),
            event("desktop", Origin::Recovered, false),
            event("live", Origin::Live, true),
            event("released", Origin::Recovered, true),
        ];
        let scope = ["phone", "desktop", "live"].map(|id| RunKey::Flow(id.into()));
        let owed: Vec<_> = phone_owed(&events, &scope)
            .map(|event| event.snapshot.key.clone())
            .collect();
        assert_eq!(owed, ["phone", "live"].map(|id| RunKey::Flow(id.into())));
    }
}
