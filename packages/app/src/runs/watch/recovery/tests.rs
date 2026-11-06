use super::super::tests::ended;
use super::*;
use crate::runs::result::RunSnapshot;

fn restored(snapshot: RunSnapshot, end: impl FnOnce(RunSnapshot) -> RestoredEnd) -> Restored {
    Restored {
        key: snapshot.key.clone(),
        solicited: true,
        location: None,
        owed: None,
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
            owed: None,
            end: RestoredEnd::Open,
        }],
    );
    watch.adopt_log(vec![key.clone()], Vec::new(), None, &Default::default());
    watch.retain(ended(key.clone()));
    let pending = watch.unreported();
    assert_eq!(pending.len(), 1);
    assert!(pending[0].solicited);
    assert_eq!(pending[0].origin, Origin::Live);
}
