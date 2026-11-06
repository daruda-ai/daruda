use super::super::tests::ended;
use super::*;
use crate::runs::result::{RunSnapshot, RunState};

fn restored(snapshot: RunSnapshot, end: impl FnOnce(RunSnapshot) -> RestoredEnd) -> Restored {
    Restored {
        key: snapshot.key.clone(),
        solicited: true,
        location: None,
        owed: Vec::new(),
        end: end(snapshot),
    }
}

#[test]
fn a_restored_end_is_one_recovered_episode_even_when_retained_again() {
    let mut watch = RunWatch::default();
    let snapshot = ended(RunKey::Flow("f".into()));
    watch.attach_ledger(
        Ledger::Off,
        vec![restored(snapshot.clone(), |snapshot| {
            RestoredEnd::Unreported {
                episode: 7,
                snapshot,
            }
        })],
    );
    watch.retain(snapshot.clone());
    let pending = watch.unreported();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].origin, Origin::Recovered);
    assert_eq!(pending[0].recorded, Some(7));
    assert!(pending[0].solicited);
    assert!(watch.recovered(&snapshot.key));
    assert_eq!(watch.scope(), vec![snapshot.key]);
}

#[test]
fn a_restored_reported_end_stays_in_scope_without_an_episode() {
    let mut watch = RunWatch::default();
    let snapshot = ended(RunKey::Flow("f".into()));
    watch.attach_ledger(
        Ledger::Off,
        vec![restored(snapshot.clone(), RestoredEnd::Reported)],
    );
    watch.retain(snapshot.clone());
    assert!(watch.unreported().is_empty());
    assert_eq!(watch.scope(), vec![snapshot.key.clone()]);
    assert!(!watch.recovered(&snapshot.key));
}

#[test]
fn a_carried_key_already_restored_keeps_the_restored_solicitation() {
    let mut watch = RunWatch::default();
    let key = RunKey::Flow("open".into());
    watch.attach_ledger(
        Ledger::Off,
        vec![Restored {
            key: key.clone(),
            solicited: true,
            location: None,
            owed: Vec::new(),
            end: RestoredEnd::Open,
        }],
    );
    watch.adopt_log(vec![key.clone()], Vec::new(), None, &Default::default());
    watch.retain(ended(key.clone()));
    let pending = watch.unreported();
    assert_eq!(pending.len(), 1);
    assert!(pending[0].solicited);
    assert_eq!(
        pending[0].origin,
        Origin::Recovered,
        "its first read judges it, whichever path made it"
    );
}

fn open_flow(key: &RunKey, owed: Vec<OwedEnd>) -> Restored {
    Restored {
        key: key.clone(),
        solicited: true,
        location: None,
        owed,
        end: RestoredEnd::Open,
    }
}

fn crashed(key: &RunKey) -> RunSnapshot {
    let mut snapshot = ended(key.clone());
    snapshot.state = RunState::Ended {
        ended_as: crate::runs::result::EndedAs::Crashed,
        text: Some("stopped".into()),
        pointers: Default::default(),
    };
    snapshot
}

/// A wait that reads a restored flow's end before the startup read does
/// settles it as recovered all the same, so the event that starts the
/// orchestrator for it is the one a judged end sends; the startup read then
/// adds nothing.
#[test]
fn a_restored_flows_end_a_wait_reads_first_is_recovered() {
    let mut watch = RunWatch::default();
    let (sender, announced) = smol::channel::unbounded();
    watch.event_sender = Some(sender);
    let key = RunKey::Flow("f".into());
    watch.attach_ledger(Ledger::Off, vec![open_flow(&key, Vec::new())]);
    assert_eq!(watch.unjudged(), vec![key.clone()]);

    watch.observe_disk(ended(key.clone()), 0);
    let event = announced.try_recv().expect("the end is announced");
    assert_eq!(event.origin, Origin::Recovered);
    assert!(
        event.solicited,
        "what starts the orchestrator after a restart"
    );
    assert!(watch.recovered(&key));
    assert!(watch.unjudged().is_empty(), "the wait's read judged it");

    watch.judge_restored(ended(key.clone()), Some(0));
    assert_eq!(watch.unreported().len(), 1);
    assert!(announced.try_recv().is_err());
}

/// A resume the restart cut off before it wrote anything leaves disk
/// showing the crash the run still owes: judging it adds no second episode,
/// and the owed end is the run's current, resumable end again.
#[test]
fn a_resume_that_never_ran_does_not_repeat_the_crash_it_owes() {
    let mut watch = RunWatch::default();
    let key = RunKey::Flow("f".into());
    let owed = OwedEnd {
        episode: 3,
        snapshot: crashed(&key),
    };
    watch.attach_ledger(Ledger::Off, vec![open_flow(&key, vec![owed])]);
    let mut read_again = crashed(&key);
    if let RunState::Ended { text, .. } = &mut read_again.state {
        *text = Some("read again".into());
    }
    watch.judge_restored(read_again, Some(4));
    let pending = watch.unreported();
    assert_eq!(pending.len(), 1, "{pending:?}");
    assert_eq!(pending[0].recorded, Some(3));
    assert!(pending[0].resumable(), "the run can be picked up again");
    assert!(watch.recovered(&key));

    let (id, _) = watch.open_wait(Some(vec![key.clone()])).unwrap();
    assert_eq!(watch.poll_wait(id, false).unwrap().ended.len(), 1);
    watch.finish_wait(id, true);
    assert!(
        watch.entries[&key].is_over(),
        "reporting it ends the run's watch"
    );
}

/// An end the resumed run reached is its own episode beside the owed one.
#[test]
fn a_resumed_runs_own_end_is_a_new_episode_beside_the_owed_crash() {
    let mut watch = RunWatch::default();
    let key = RunKey::Flow("f".into());
    let owed = OwedEnd {
        episode: 3,
        snapshot: crashed(&key),
    };
    watch.attach_ledger(Ledger::Off, vec![open_flow(&key, vec![owed])]);
    watch.judge_restored(ended(key.clone()), Some(4));
    let pending = watch.unreported();
    assert_eq!(pending.len(), 2, "{pending:?}");
    assert!(!pending[0].resumable(), "the owed crash was resumed past");
    assert_eq!(pending[1].origin, Origin::Recovered);
}
