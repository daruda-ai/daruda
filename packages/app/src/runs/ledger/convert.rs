//! The run-owned types and their store-level twins the ledger records.

use daruda_store::run_ledger::{
    LedgerEnd, LedgerEndedAs, LedgerKey, LedgerPointers, RunLedgerEvent, RunLocation,
};

use crate::runs::result::{EndedAs, FlowRunLocation, Pointers, RunKey, RunSnapshot, RunState};

pub(super) fn ledger_key(key: &RunKey) -> LedgerKey {
    match key {
        RunKey::Flow(run_id) => LedgerKey::Flow {
            run_id: run_id.clone(),
        },
        RunKey::Task(exec) => LedgerKey::Task(exec.clone()),
    }
}

pub(super) fn key_of(key: &LedgerKey) -> RunKey {
    match key {
        LedgerKey::Flow { run_id } => RunKey::Flow(run_id.clone()),
        LedgerKey::Task(exec) => RunKey::Task(exec.clone()),
    }
}

pub(super) fn ledger_key_of(event: &RunLedgerEvent) -> &LedgerKey {
    match event {
        RunLedgerEvent::Owned { key, .. }
        | RunLedgerEvent::Reopened { key, .. }
        | RunLedgerEvent::Ended { key, .. }
        | RunLedgerEvent::Reported { key, .. }
        | RunLedgerEvent::Released { key, .. } => key,
    }
}

pub(super) fn run_location(location: &FlowRunLocation) -> RunLocation {
    RunLocation {
        run_dir: location.run_dir.clone(),
        lock_dir: location.lock_dir.clone(),
    }
}

pub(super) fn location_of(location: &RunLocation) -> FlowRunLocation {
    FlowRunLocation {
        run_dir: location.run_dir.clone(),
        lock_dir: location.lock_dir.clone(),
    }
}

pub(super) fn ledger_end(snapshot: &RunSnapshot) -> Option<LedgerEnd> {
    let RunState::Ended {
        ended_as,
        text,
        pointers,
    } = &snapshot.state
    else {
        return None;
    };
    Some(LedgerEnd {
        what: snapshot.what.clone(),
        why: snapshot.why.clone(),
        ended_as: match ended_as {
            EndedAs::Completed => LedgerEndedAs::Completed,
            EndedAs::Failed => LedgerEndedAs::Failed,
            EndedAs::Canceled => LedgerEndedAs::Canceled,
            EndedAs::Stalled => LedgerEndedAs::Stalled,
            EndedAs::Crashed => LedgerEndedAs::Crashed,
            EndedAs::Interrupted => LedgerEndedAs::Interrupted,
            EndedAs::Undelivered => LedgerEndedAs::Undelivered,
            EndedAs::Error => LedgerEndedAs::Error,
            EndedAs::BudgetExhausted => LedgerEndedAs::BudgetExhausted,
        },
        text: text.clone(),
        pointers: LedgerPointers {
            run_dir: pointers.run_dir.clone(),
            report: pointers.report.clone(),
            outputs: pointers.outputs.clone(),
            chat: pointers.chat.clone(),
            transcript: pointers.transcript.clone(),
        },
    })
}

pub(super) fn snapshot_of(key: RunKey, end: &LedgerEnd) -> RunSnapshot {
    RunSnapshot {
        key,
        what: end.what.clone(),
        why: end.why.clone(),
        state: RunState::Ended {
            ended_as: match end.ended_as {
                LedgerEndedAs::Completed => EndedAs::Completed,
                LedgerEndedAs::Failed => EndedAs::Failed,
                LedgerEndedAs::Canceled => EndedAs::Canceled,
                LedgerEndedAs::Stalled => EndedAs::Stalled,
                LedgerEndedAs::Crashed => EndedAs::Crashed,
                LedgerEndedAs::Interrupted => EndedAs::Interrupted,
                LedgerEndedAs::Undelivered => EndedAs::Undelivered,
                LedgerEndedAs::Error => EndedAs::Error,
                LedgerEndedAs::BudgetExhausted => EndedAs::BudgetExhausted,
            },
            text: end.text.clone(),
            pointers: Pointers {
                run_dir: end.pointers.run_dir.clone(),
                report: end.pointers.report.clone(),
                outputs: end.pointers.outputs.clone(),
                chat: end.pointers.chat.clone(),
                transcript: end.pointers.transcript.clone(),
            },
        },
    }
}

#[cfg(test)]
mod tests;
