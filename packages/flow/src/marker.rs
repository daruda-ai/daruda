//! How a run ended, written as a file, and read back as a status.
//!
//! A marker is a cache of the run log's `RunEnded`, written after it; when
//! the two disagree the log wins. A crash cannot write a record of itself,
//! so `Crashed` is derived from an open log whose lock is gone or names a
//! dead holder, rather than from a marker of its own.

use crate::error::{FlowIoError, IoSite};
use crate::lock::IsAlive;
use crate::schedule::RunOutcome;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// The marker file names, which are also the vocabulary a user reading the
/// run directory sees.
const DONE: &str = "DONE";
const FAILED: &str = "FAILED";
const CANCELED: &str = "CANCELED";
/// Written like the three above, but read like `Crashed`: the run said how
/// it ended, and what it said is "not finished".
const STALLED: &str = "STALLED";
pub(crate) const MARKER_NAMES: [&str; 4] = [DONE, FAILED, CANCELED, STALLED];

/// Not a status: an app that still owes someone this run's result holds its
/// directory from retention until the Unix-millisecond deadline it contains.
/// Retention in any app instance reads it, so a profile that never sees the
/// holder's own records still spares the run.
const HELD: &str = "HELD";

const WRITE_MARKER: &str = "recording how the run ended";
const CLEAR_MARKER: &str = "clearing how the run last ended";

/// The four a run can write about itself, and the three that can only be
/// derived. `Crashed` has no marker because a crash cannot write one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    Done,
    Failed,
    Canceled,
    /// The scheduler could not place a node and nothing else could start.
    /// Its own status because it is resumable and `Failed` is not: no node
    /// failed, no policy gave up, the journal is complete and nothing was
    /// in flight — a cleaner place to pick up from than a crash.
    Stalled,
    Running,
    Crashed,
    Unknown,
}

/// Record how the run ended, in its own run directory.
///
/// Must happen before the lock is released: in the window between a freed
/// lock and an unwritten marker, a reader sees neither and calls a finished
/// run `Unknown`.
pub fn write_marker(run_dir: &Path, outcome: &RunOutcome) -> Result<(), FlowIoError> {
    let Some(name) = marker_name(outcome) else {
        return Ok(());
    };
    let path = run_dir.join(name);
    std::fs::create_dir_all(run_dir)
        // Empty: the name is the whole message, and why a run failed is
        // `run.md`'s (P2b-3) to say.
        .and_then(|()| std::fs::write(&path, ""))
        .map_err(|source| FlowIoError {
            site: IoSite::Run,
            doing: WRITE_MARKER,
            path,
            source,
        })
}

/// How a finished run classifies, coarsely. `Io` and `BudgetExhausted` are
/// `Failed` too — they did not succeed, and which of them it was is `RunEnd`'s
/// finer axis to carry. `None` is `LockHeld`: that run never took the
/// directory, so it has nothing to say about it.
///
/// The one place every outcome is folded onto this coarser axis.
/// `event::RunEnd` does not repeat the fold — it spreads the same outcomes
/// out instead, so a new one is decided once on each axis rather than twice
/// on one.
pub fn status_of(outcome: &RunOutcome) -> Option<RunStatus> {
    match outcome {
        RunOutcome::Done => Some(RunStatus::Done),
        RunOutcome::Failed { .. }
        | RunOutcome::BudgetExhausted { .. }
        | RunOutcome::Io(_)
        | RunOutcome::Log(_)
        | RunOutcome::Unprovisioned { .. } => Some(RunStatus::Failed),
        // Not folded into `Failed`: that would make it unresumable, and a
        // run that stopped because a directory would not resolve has lost
        // nothing a resume cannot pick up. It still owes a marker — without
        // one the next reader finds a lockless run directory and guesses
        // `Crashed`, which is the same verb by a wrong route.
        RunOutcome::Stalled { .. } => Some(RunStatus::Stalled),
        RunOutcome::Canceled { .. } => Some(RunStatus::Canceled),
        // Neither took the directory, so neither has anything to say
        // about it.
        RunOutcome::LockHeld { .. } | RunOutcome::Invalid { .. } => None,
        // Refused under the lock before anything changed: how the run last
        // ended still stands.
        RunOutcome::NotResumed(_) => None,
        // No marker, like a crash: the evidence a resume reads is the log
        // and a lock whose holder is gone.
        RunOutcome::ShutDown => None,
    }
}

/// The file name a status is written under. `Running`, `Crashed` and
/// `Unknown` have none — they are read from evidence, never written, so no
/// outcome produces them.
fn marker_name(outcome: &RunOutcome) -> Option<&'static str> {
    match status_of(outcome)? {
        RunStatus::Done => Some(DONE),
        RunStatus::Failed => Some(FAILED),
        RunStatus::Canceled => Some(CANCELED),
        RunStatus::Stalled => Some(STALLED),
        RunStatus::Running | RunStatus::Crashed | RunStatus::Unknown => None,
    }
}

/// What state a run directory is in. With no marker the lock is the only
/// evidence there is, so where the lock lives is part of this answer.
///
/// INVARIANT: `lock_dir` is passed, not derived, and naming the wrong one
/// reads every live run as `Unknown` — no marker, no holder, nothing
/// resumable — with neither the compiler nor a file-level test saying so.
/// `None` says the caller does not know where it is. `crate::resume` has
/// the test that walks the whole path.
pub fn run_status(run_dir: &Path, lock_dir: Option<&Path>, is_alive: IsAlive<'_>) -> RunStatus {
    let open = match crate::run_log::reader::ending(run_dir) {
        crate::run_log::reader::Ending::Ended(kind) => return status_of_end(kind),
        crate::run_log::reader::Ending::Open => true,
        // Not seen to end, so read like an open one: with its writer gone it
        // is offered for resume, and the resume says why it cannot be read.
        crate::run_log::reader::Ending::Unreadable => true,
        crate::run_log::reader::Ending::NoLog => false,
    };
    if let Some(status) = marked(run_dir) {
        return status;
    }
    // A log is only ever written under the lock, so an open log with no
    // lock file left means its writer is gone: a refused resume or a
    // panicking run thread gives the lock back without ending the log.
    if open && lock_dir.is_some_and(crate::lock::is_absent) {
        return RunStatus::Crashed;
    }
    lock_status(run_dir, lock_dir, is_alive)
}

fn status_of_end(kind: crate::run_log::machine::EndKind) -> RunStatus {
    use crate::run_log::machine::EndKind;
    match kind {
        EndKind::Done => RunStatus::Done,
        EndKind::Failed => RunStatus::Failed,
        EndKind::Canceled => RunStatus::Canceled,
        EndKind::Stalled => RunStatus::Stalled,
    }
}

/// What the markers say, for a run whose log has no ending to give: one an
/// older build ran, or one whose log failed before it could record its end.
fn marked(run_dir: &Path) -> Option<RunStatus> {
    if run_dir.join(DONE).is_file() {
        return Some(RunStatus::Done);
    }
    if run_dir.join(FAILED).is_file() {
        return Some(RunStatus::Failed);
    }
    if run_dir.join(CANCELED).is_file() {
        return Some(RunStatus::Canceled);
    }
    if run_dir.join(STALLED).is_file() {
        return Some(RunStatus::Stalled);
    }
    None
}

fn lock_status(run_dir: &Path, lock_dir: Option<&Path>, is_alive: IsAlive<'_>) -> RunStatus {
    // `None` is a caller that could not resolve the tree, so it does not
    // know where the lock is — said as an absent argument rather than as a
    // path with no lock in it, which would read the same as "free".
    let holder = lock_dir.and_then(crate::lock::read_holder);
    // A lock naming a different run is evidence about that run, not this
    // one, and while it is held no resume could take the tree anyway; an
    // unreadable or absent one is no evidence at all. Either way there is
    // nothing here to call a crash without guessing.
    match holder {
        Some(holder) if !holds_this_run(&holder.run_id, run_dir) => RunStatus::Unknown,
        Some(holder) if is_alive(holder.pid) => RunStatus::Running,
        Some(_) => RunStatus::Crashed,
        None => RunStatus::Unknown,
    }
}

/// Remove every marker, so a resumed run no longer reads as how it last
/// ended. A marker already gone is not an error.
pub(crate) fn clear_markers(run_dir: &Path) -> Result<(), FlowIoError> {
    for name in MARKER_NAMES {
        let path = run_dir.join(name);
        match std::fs::remove_file(&path) {
            Err(source) if source.kind() != std::io::ErrorKind::NotFound => {
                return Err(FlowIoError {
                    site: IoSite::Run,
                    doing: CLEAR_MARKER,
                    path,
                    source,
                });
            }
            _ => {}
        }
    }
    Ok(())
}

/// The run's id is the name of its directory — the host builds
/// `<cwd>/.daruda/flow-runs/<run-id>/`, so there is nothing else to compare.
fn holds_this_run(run_id: &str, run_dir: &Path) -> bool {
    run_dir.file_name().is_some_and(|name| name == run_id)
}

/// How many finished runs a sweep leaves behind when the host states no
/// preference. The parameter is where a host that gains a config knob
/// expresses its own.
pub const DEFAULT_KEEP_RUNS: usize = 20;

/// Delete finished runs beyond `keep`, newest first, and report what went.
///
/// "Finished" means a completion marker is present — not `run_status`, which
/// also answers from the lock and would call a crashed run `Unknown` whenever
/// any other run is going. An unmarked directory is the only evidence a crash
/// ever happened, so
/// it is never a candidate however old it is, and never counts against `keep`.
/// Reading no lock is why this takes no `is_alive`.
///
/// Newest is by directory name, chronological only because the host names
/// run directories with ULIDs — another id scheme would silently delete
/// the wrong ones.
///
/// `held` names runs, by id (the directory name), whose directories the host
/// still needs: they are neither deleted nor counted against `keep`. A run
/// whose [`hold`] marker has not reached its deadline is spared the same way.
pub fn sweep_old_runs(
    runs_dir: &Path,
    keep: usize,
    held: &HashSet<String>,
) -> std::io::Result<Vec<PathBuf>> {
    let now = now_ms();
    let mut finished: Vec<PathBuf> = std::fs::read_dir(runs_dir)?
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_none_or(|name| !held.contains(name))
        })
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && has_marker(path))
        .filter(|path| !held_until_later(path, now))
        .collect();
    finished.sort();

    let over = finished.len().saturating_sub(keep);
    let mut removed = Vec::with_capacity(over);
    for run_dir in finished.into_iter().take(over) {
        std::fs::remove_dir_all(&run_dir)?;
        removed.push(run_dir);
    }
    Ok(removed)
}

/// Hold `run_dir` from every app instance's retention until `until_ms`, in
/// Unix milliseconds. Never creates the directory: a run already gone is an
/// error the caller may ignore.
pub fn hold(run_dir: &Path, until_ms: u64) -> std::io::Result<()> {
    if !run_dir.is_dir() {
        return Err(std::io::ErrorKind::NotFound.into());
    }
    std::fs::write(run_dir.join(HELD), until_ms.to_string())
}

/// Take back a [`hold`]; a run held by none, or gone, is no error.
pub fn release_hold(run_dir: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(run_dir.join(HELD)) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Whether a [`hold`] in `run_dir` still stands at `now_ms`. A deadline that
/// cannot be read holds nothing, so a torn write never pins a run forever.
fn held_until_later(run_dir: &Path, now_ms: u64) -> bool {
    std::fs::read_to_string(run_dir.join(HELD))
        .ok()
        .and_then(|until| until.trim().parse::<u64>().ok())
        .is_some_and(|until| now_ms < until)
}

/// The clock holds are judged against, in Unix milliseconds.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

/// Whether the run is finished *and* done with. The one question retention
/// asks — and the only one it can answer without a lock.
///
/// This is what keeps a run that can still be picked up around long enough
/// to be: a killed run never wrote a marker at all, and a stalled one wrote
/// `STALLED`, which says it ended without finishing. Neither is a sweep
/// candidate however old. See [`crate::resume`].
fn has_marker(run_dir: &Path) -> bool {
    [DONE, FAILED, CANCELED]
        .iter()
        .any(|name| run_dir.join(name).is_file())
}

#[cfg(test)]
mod tests;
