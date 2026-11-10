use super::super::book::{MAX_AGE_SECS, MAX_SESSIONS, MAX_THREADS, TOUCH_INTERVAL_SECS};
use super::*;

#[test]
fn a_thread_is_found_only_under_the_identity_that_made_it() {
    let mut book = Book::default();
    book.insert(&work(), "t".into(), key("claude", "s1"), "1.1".into(), NOW);
    let found = |scope: &Scope| {
        book.for_conversation(scope, &key("claude", "s1"))
            .map(|r| r.root.clone())
    };
    let cases = [
        (work(), Some("1.1".to_owned()), "same identity"),
        (scope("home", "bot", "D1"), None, "another connection"),
        (scope("work", "other", "D1"), None, "another bot"),
        (scope("work", "bot", "D2"), None, "another chat"),
    ];
    for (scope, want, label) in cases {
        assert_eq!(found(&scope), want, "{label}");
    }
    assert!(
        book.for_conversation(&work(), &key("codex", "s1"))
            .is_none(),
        "another agent"
    );
}

#[test]
fn a_conversation_belongs_to_one_thread() {
    let mut book = Book::default();
    book.insert(&work(), "t".into(), key("claude", "s1"), "1.1".into(), NOW);
    book.insert(&work(), "t".into(), key("claude", "s1"), "2.2".into(), NOW);
    assert_eq!(book.records().len(), 1, "the older claim is released");
    assert_eq!(book.records()[0].root, "2.2");
}

#[test]
fn a_new_conversation_continues_the_previous_thread() {
    let mut book = Book::default();
    book.insert(&work(), "t".into(), key("claude", "s1"), "1.1".into(), NOW);
    // Another thread already holds the conversation being continued into.
    book.insert(&work(), "t".into(), key("claude", "s2"), "9.9".into(), NOW);

    let root = book.continue_with(&work(), &key("claude", "s1"), key("claude", "s2"), NOW + 5);

    assert_eq!(root.as_deref(), Some("1.1"));
    assert_eq!(book.records().len(), 1, "the emptied thread is dropped");
    assert_eq!(
        book.records()[0].sessions,
        vec![key("claude", "s1"), key("claude", "s2")]
    );
    assert_eq!(book.records()[0].last_used, NOW + 5);
    assert_eq!(
        book.continue_with(&work(), &key("claude", "unknown"), key("claude", "s3"), NOW),
        None,
        "nothing to continue"
    );
    assert_eq!(
        book.continue_with(&work(), &key("claude", "s2"), key("claude", "s2"), NOW),
        None,
        "same conversation"
    );
}

#[test]
fn a_thread_keeps_only_its_recent_conversations() {
    let mut book = Book::default();
    book.insert(&work(), "t".into(), key("claude", "0"), "1.1".into(), NOW);
    for n in 1..MAX_SESSIONS + 5 {
        book.continue_with(
            &work(),
            &key("claude", &(n - 1).to_string()),
            key("claude", &n.to_string()),
            NOW,
        );
    }
    let sessions = &book.records()[0].sessions;
    assert_eq!(sessions.len(), MAX_SESSIONS);
    assert_eq!(
        sessions.last(),
        Some(&key("claude", &(MAX_SESSIONS + 4).to_string()))
    );
}

#[test]
fn use_is_saved_at_most_once_per_interval() {
    let mut book = Book::default();
    book.insert(&work(), "t".into(), key("claude", "s1"), "1.1".into(), NOW);
    assert!(!book.touch(&work(), "1.1", NOW + TOUCH_INTERVAL_SECS - 1));
    assert!(book.touch(&work(), "1.1", NOW + TOUCH_INTERVAL_SECS));
    assert!(!book.touch(&work(), "unknown", NOW + 10 * TOUCH_INTERVAL_SECS));
}

#[test]
fn pruning_drops_stale_threads_then_keeps_the_newest() {
    let mut book = Book::default();
    book.insert(
        &work(),
        "t".into(),
        key("claude", "old"),
        "0.0".into(),
        NOW - MAX_AGE_SECS - 1,
    );
    for n in 0..MAX_THREADS + 3 {
        book.insert(
            &work(),
            "t".into(),
            key("claude", &n.to_string()),
            n.to_string(),
            NOW - n as i64,
        );
    }
    assert!(book.prune(NOW));
    assert_eq!(book.records().len(), MAX_THREADS);
    assert!(
        book.records().iter().all(|r| r.root != "0.0"),
        "past the age limit"
    );
    assert!(
        book.records()
            .iter()
            .all(|r| r.last_used > NOW - MAX_THREADS as i64),
        "only the newest survive"
    );
    assert!(!book.prune(NOW), "nothing left to drop");
}
