//! Remote thread records persist per data directory and ignore unknown schemas.

use daruda_store::remote_threads::*;

fn record(id: &str) -> ThreadRecord {
    ThreadRecord {
        id: id.into(),
        connection: "work".into(),
        bot: "digest".into(),
        conversation: "D1".into(),
        root: "1700000000.000100".into(),
        sessions: vec![ConversationKey {
            agent: "claude".into(),
            session: "s-1".into(),
        }],
        last_used: 1_700_000_000,
    }
}

#[test]
fn records_round_trip_in_their_own_directory() {
    let dir = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let store = RemoteThreadStore::at(dir.path());
    assert!(store.load().is_empty());
    store.save(&[record("a"), record("b")]).unwrap();
    assert_eq!(store.load(), vec![record("a"), record("b")]);
    assert!(RemoteThreadStore::at(other.path()).load().is_empty());
}

#[test]
fn an_unknown_schema_or_corrupt_file_reads_as_empty() {
    for bytes in [&b"{\"version\":99,\"threads\":[]}"[..], &b"{not json"[..]] {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("remote-threads.json"), bytes).unwrap();
        assert!(RemoteThreadStore::at(dir.path()).load().is_empty());
    }
}
