/// These pass the run directory's parent as `run_status`'s `lock_dir`,
/// so they read its **primary** path — at a location that happens to be
/// where the compatibility copy also lives. The fallback never runs
/// here, because the primary answers first; `crate::resume` has the
/// test that covers it. The lock's real home outside the tree has tests
/// of its own below.
fn status_beside(run_dir: &std::path::Path, is_alive: super::IsAlive<'_>) -> super::RunStatus {
    super::run_status(run_dir, run_dir.parent(), is_alive)
}

use super::*;
use crate::lock::LockHolder;
use crate::runner::NodeFailure;
use crate::schedule::{BudgetLimit, RunOutcome};
use std::path::{Path, PathBuf};

fn run_dir_in(dir: &Path, run_id: &str) -> PathBuf {
    let run_dir = dir.join(run_id);
    std::fs::create_dir_all(&run_dir).expect("mkdir");
    run_dir
}

/// Plant a `.lock` naming a holder this process is not — the only way
/// to stage a dead pid without killing anything.
fn write_lock(dir: &Path, pid: u32, run_id: &str) {
    let holder = LockHolder {
        pid,
        run_id: run_id.to_string(),
        started_unix_secs: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    };
    std::fs::write(
        dir.join(".lock"),
        yaml_serde::to_string(&holder).expect("serialise"),
    )
    .expect("write");
}

/// Every marker a run can write, and the outcome that writes it. The
/// several ways of failing share `FAILED`: `run.md` (P2b-3) is where the
/// reason belongs.
#[test]
fn each_outcome_a_run_can_record_writes_its_own_marker() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cases: [(RunOutcome, &str, RunStatus); 6] = [
        (RunOutcome::Done, "DONE", RunStatus::Done),
        (
            RunOutcome::Failed {
                node: "design".into(),
                failure: NodeFailure::Exit { code: Some(1) },
            },
            "FAILED",
            RunStatus::Failed,
        ),
        (
            RunOutcome::BudgetExhausted {
                limit: BudgetLimit::NodeRuns,
            },
            "FAILED",
            RunStatus::Failed,
        ),
        (
            RunOutcome::Canceled {
                node: Some("design".into()),
            },
            "CANCELED",
            RunStatus::Canceled,
        ),
        (
            RunOutcome::Canceled { node: None },
            "CANCELED",
            RunStatus::Canceled,
        ),
        (
            RunOutcome::Stalled {
                nodes: vec!["design".into()],
            },
            "STALLED",
            RunStatus::Stalled,
        ),
    ];
    for (outcome, name, status) in cases {
        let run_dir = run_dir_in(dir.path(), name);
        write_marker(&run_dir, &outcome).expect("write");
        assert!(run_dir.join(name).is_file(), "{outcome:?}");
        assert_eq!(status_beside(&run_dir, &|_| true), status);
        std::fs::remove_file(run_dir.join(name)).expect("clean");
    }
}

/// A run that never took the directory has nothing to say about it —
/// stamping one would overwrite the status of the run that is going.
#[test]
fn a_run_that_never_took_the_lock_writes_no_marker() {
    let dir = tempfile::tempdir().expect("tempdir");
    let run_dir = run_dir_in(dir.path(), "01J");
    write_marker(
        &run_dir,
        &RunOutcome::LockHeld {
            holder: LockHolder {
                pid: 4242,
                run_id: "other".to_string(),
                started_unix_secs: 0,
            },
        },
    )
    .expect("write");
    assert_eq!(std::fs::read_dir(&run_dir).expect("read").count(), 0);
}

/// The marker's directory may not exist yet: a command-only flow writes
/// no output, so nothing else has created it.
#[test]
fn a_marker_creates_its_run_directory() {
    let dir = tempfile::tempdir().expect("tempdir");
    let run_dir = dir.path().join("never-created");
    write_marker(&run_dir, &RunOutcome::Done).expect("write");
    assert!(run_dir.join("DONE").is_file());
}

#[test]
fn no_marker_and_a_live_holder_reads_as_running() {
    let dir = tempfile::tempdir().expect("tempdir");
    let run_dir = run_dir_in(dir.path(), "01J");
    write_lock(dir.path(), 4242, "01J");
    assert_eq!(status_beside(&run_dir, &|_| true), RunStatus::Running);
}

/// The row that cannot be reached by writing a marker: a crash leaves no
/// record of itself, so the absence of one plus a dead holder is the only
/// evidence there is.
#[test]
fn no_marker_and_a_dead_holder_reads_as_crashed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let run_dir = run_dir_in(dir.path(), "01J");
    write_lock(dir.path(), 4242, "01J");
    assert_eq!(status_beside(&run_dir, &|_| false), RunStatus::Crashed);
}

/// Not the same as crashed. Without a lock there is no evidence either
/// way, and saying "crashed" would be a guess presented as a fact.
#[test]
fn no_marker_and_no_lock_reads_as_unknown() {
    let dir = tempfile::tempdir().expect("tempdir");
    let run_dir = run_dir_in(dir.path(), "01J");
    assert_eq!(status_beside(&run_dir, &|_| true), RunStatus::Unknown);
}

/// A live lock naming a different run is evidence about that run, not
/// this one. Reading it as `Running` would report every unmarked run in
/// the history as live for as long as any run is going.
#[test]
fn a_lock_held_by_another_run_says_nothing_about_this_one() {
    let dir = tempfile::tempdir().expect("tempdir");
    let run_dir = run_dir_in(dir.path(), "01J");
    write_lock(dir.path(), 4242, "01K");
    assert_eq!(status_beside(&run_dir, &|_| true), RunStatus::Unknown);
}

/// A run that ended and said so. The id doubles as the sort key, the way
/// a ULID does.
fn finished_run_in(dir: &Path, run_id: &str) -> PathBuf {
    let run_dir = run_dir_in(dir, run_id);
    std::fs::write(run_dir.join(DONE), "").expect("marker");
    run_dir
}

/// The directory names left behind, sorted — what a user would see.
fn surviving(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("read")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// The rule that makes retention safe: a directory with no marker is the
/// only evidence a crash ever happened.
///
/// The counts matter. 25 marked — five over the cap — proves the cap is
/// enforced at all; with exactly 20 a sweep that never ran would pass.
/// The 3 unmarked are older than every marked one, so surviving proves
/// they are exempt rather than merely fitting under the cap.
#[test]
fn a_sweep_keeps_unmarked_runs_however_old_they_are() {
    let dir = tempfile::tempdir().expect("tempdir");
    for i in 0..3 {
        run_dir_in(dir.path(), &format!("01A{i:02}"));
    }
    for i in 0..25 {
        finished_run_in(dir.path(), &format!("01B{i:02}"));
    }

    let removed = sweep_old_runs(dir.path(), 20, &HashSet::new()).expect("sweep");

    assert_eq!(removed.len(), 5, "{removed:?}");
    let left = surviving(dir.path());
    assert_eq!(left.len(), 23, "{left:?}");
    for i in 0..3 {
        let unmarked = format!("01A{i:02}");
        assert!(left.contains(&unmarked), "{unmarked} was swept: {left:?}");
    }
}

/// The other half of the retention rule, and the one a tidy-up is most
/// likely to undo: `STALLED` *is* a marker, so adding it to `has_marker`
/// alongside the other three reads as an omission being corrected — and
/// deletes runs a resume could still have finished. The counts are the
/// sibling test's, for the same reasons.
#[test]
fn a_sweep_keeps_stalled_runs_however_old_they_are() {
    let dir = tempfile::tempdir().expect("tempdir");
    for i in 0..3 {
        let run_dir = run_dir_in(dir.path(), &format!("01A{i:02}"));
        std::fs::write(run_dir.join(STALLED), "").expect("marker");
    }
    for i in 0..25 {
        finished_run_in(dir.path(), &format!("01B{i:02}"));
    }

    let removed = sweep_old_runs(dir.path(), 20, &HashSet::new()).expect("sweep");

    assert_eq!(removed.len(), 5, "{removed:?}");
    let left = surviving(dir.path());
    assert_eq!(left.len(), 23, "{left:?}");
    for i in 0..3 {
        let stalled = format!("01A{i:02}");
        assert!(left.contains(&stalled), "{stalled} was swept: {left:?}");
    }
}

/// The case that catches a sweep keyed on `run_status` instead of the
/// marker: with a live lock naming some other run, an unmarked directory
/// reads as `Unknown` rather than `Crashed` — and any run at all being in
/// progress is enough to produce that. The evidence of the crash must
/// survive regardless.
#[test]
fn a_sweep_keeps_an_unmarked_run_while_another_run_holds_the_lock() {
    let dir = tempfile::tempdir().expect("tempdir");
    let crashed = run_dir_in(dir.path(), "01A00");
    for i in 0..25 {
        finished_run_in(dir.path(), &format!("01B{i:02}"));
    }
    // Beside the run directories, which is where a `run_status`-keyed
    // sweep would look — it has only `runs_dir` to offer as the lock's
    // directory. This one reads no lock at all.
    write_lock(dir.path(), 4242, "other");
    assert_eq!(
        status_beside(&crashed, &|_| true),
        RunStatus::Unknown,
        "the misreading this test exists to survive"
    );

    let removed = sweep_old_runs(dir.path(), 20, &HashSet::new()).expect("sweep");

    assert!(crashed.is_dir(), "the only evidence of a crash was swept");
    assert!(!removed.contains(&crashed), "{removed:?}");
}

/// A run the host still holds outlives the cap without displacing a
/// newer one: 25 finished beside 3 older held ones lose exactly the 5
/// oldest unheld, so the held three were neither deleted nor counted.
#[test]
fn a_sweep_neither_deletes_nor_counts_held_runs() {
    let dir = tempfile::tempdir().expect("tempdir");
    let held: HashSet<String> = (0..3).map(|i| format!("01A{i:02}")).collect();
    for name in &held {
        finished_run_in(dir.path(), name);
    }
    for i in 0..25 {
        finished_run_in(dir.path(), &format!("01B{i:02}"));
    }

    let removed = sweep_old_runs(dir.path(), 20, &held).expect("sweep");

    assert_eq!(removed.len(), 5, "{removed:?}");
    let left = surviving(dir.path());
    assert_eq!(left.len(), 23, "{left:?}");
    for name in &held {
        assert!(left.contains(name), "held {name} was swept: {left:?}");
    }
    assert!(left.contains(&"01B05".to_string()), "{left:?}");
}

/// A hold another app instance left in the run directory spares it until
/// its deadline, like a held id; a passed or unreadable deadline, or one
/// taken back, spares nothing, so a hold can never pin a run forever.
#[test]
fn a_sweep_spares_a_run_holding_a_marker_until_its_deadline() {
    let dir = tempfile::tempdir().expect("tempdir");
    let now = now_ms();
    let held = finished_run_in(dir.path(), "01A00");
    hold(&held, now + 60_000).expect("hold");
    let lapsed = finished_run_in(dir.path(), "01A01");
    hold(&lapsed, now.saturating_sub(1)).expect("hold");
    let garbled = finished_run_in(dir.path(), "01A02");
    std::fs::write(garbled.join(HELD), "soon").expect("marker");
    let released = finished_run_in(dir.path(), "01A03");
    hold(&released, now + 60_000).expect("hold");
    release_hold(&released).expect("release");
    release_hold(&released).expect("releasing twice is no error");
    for i in 0..20 {
        finished_run_in(dir.path(), &format!("01B{i:02}"));
    }

    let removed = sweep_old_runs(dir.path(), 20, &HashSet::new()).expect("sweep");

    assert_eq!(removed, [lapsed, garbled, released.clone()]);
    assert!(held.is_dir());
    assert!(
        hold(&dir.path().join("01Z99"), now).is_err(),
        "a hold never creates the directory it names"
    );
}

/// Newest kept, oldest dropped — and the ordering is by run-id, because
/// the design's run-ids are ULIDs, which sort by time.
#[test]
fn a_sweep_keeps_the_newest_and_drops_the_oldest() {
    let dir = tempfile::tempdir().expect("tempdir");
    for i in 0..22 {
        finished_run_in(dir.path(), &format!("01B{i:02}"));
    }

    let removed = sweep_old_runs(dir.path(), 20, &HashSet::new()).expect("sweep");

    let gone: Vec<String> = removed
        .iter()
        .map(|p| {
            p.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(gone, vec!["01B00".to_string(), "01B01".to_string()]);
    assert_eq!(surviving(dir.path()).len(), 20);
    assert!(dir.path().join("01B21").is_dir(), "the newest must stay");
}

/// A log that holds `RunStarted` and never ended, as a crash leaves one.
fn open_log_in(run_dir: &Path) {
    let mut log = crate::run_log::store::open_file_log(run_dir)
        .unwrap_or_else(|e| panic!("the log opens: {e:?}"));
    log.commit(crate::run_log::machine::FlowRunEvent::RunStarted {
        inputs: Default::default(),
        started_by: None,
        nodes: Vec::new(),
        until: None,
        pinned: Vec::new(),
        profile: None,
    })
    .expect("commit");
}

/// A log that never ended with no lock left: whatever stopped writing
/// it gave the lock back, and no live writer runs without one.
#[test]
fn an_open_log_with_no_lock_reads_as_crashed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let run_dir = run_dir_in(dir.path(), "01J");
    open_log_in(&run_dir);
    assert_eq!(status_beside(&run_dir, &|_| true), RunStatus::Crashed);
}

/// Where the lock is, or whose it is, still decides: an unknown lock
/// directory and a lock another run holds say nothing about this run.
#[test]
fn an_open_log_still_needs_this_runs_lock_to_be_read_as_gone() {
    let dir = tempfile::tempdir().expect("tempdir");
    let run_dir = run_dir_in(dir.path(), "01J");
    open_log_in(&run_dir);
    assert_eq!(run_status(&run_dir, None, &|_| false), RunStatus::Unknown);
    write_lock(dir.path(), 4242, "01K");
    assert_eq!(status_beside(&run_dir, &|_| false), RunStatus::Unknown);
    write_lock(dir.path(), 4242, "01J");
    assert_eq!(status_beside(&run_dir, &|_| true), RunStatus::Running);
    std::fs::write(dir.path().join(".lock"), "{ truncated").expect("write");
    assert_eq!(status_beside(&run_dir, &|_| true), RunStatus::Unknown);
}

/// A truncated or garbage lock is not evidence of a live run — the same
/// reading `RunLock::acquire` makes when it reclaims one.
#[test]
fn an_unparseable_lock_reads_as_unknown() {
    let dir = tempfile::tempdir().expect("tempdir");
    let run_dir = run_dir_in(dir.path(), "01J");
    std::fs::write(dir.path().join(".lock"), "{ truncated").expect("write");
    assert_eq!(status_beside(&run_dir, &|_| true), RunStatus::Unknown);
}
