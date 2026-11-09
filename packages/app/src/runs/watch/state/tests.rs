use super::super::tests::ended;
use super::*;

#[test]
fn solicited_registration_stamps_a_fast_cached_task_completion_before_announcement() {
    let mut watch = RunWatch::default();
    let key = RunKey::Task(daruda_store::tasks::ExecutionRef {
        task_id: "task".into(),
        execution_id: "fast".into(),
    });
    watch.retain(ended(key.clone()));
    assert!(watch.unreported().is_empty());
    let (sender, receiver) = smol::channel::unbounded();
    watch.event_sender = Some(sender);
    watch.register_solicited(key.clone(), true);
    assert!(watch.pending(&key).unwrap().solicited);
    assert!(receiver.try_recv().unwrap().solicited);
}

#[test]
fn retaining_an_identical_end_bumps_no_revision_and_wakes_no_waiter() {
    let mut watch = RunWatch::default();
    let key = RunKey::Flow("same".into());
    watch.register(key.clone());
    watch.retain(ended(key.clone()));
    let revision = watch.revision(&key);
    let (_wait, wake) = watch
        .open_wait(Some(vec![RunKey::Flow("other".into())]))
        .unwrap();
    watch.retain(ended(key.clone()));
    assert_eq!(watch.revision(&key), revision);
    assert!(wake.try_recv().is_err());
    let mut changed = ended(key.clone());
    changed.what = "changed".into();
    watch.retain(changed);
    assert!(watch.revision(&key) > revision);
    assert_eq!(
        watch.unreported().len(),
        1,
        "a changed end is not a second episode"
    );
}
