use super::book::{MAX_AGE_SECS, MAX_SESSIONS, MAX_THREADS, TOUCH_INTERVAL_SECS};
use super::driver::{Threads, drop_header_under_root};
use super::*;
use crate::remote_channel::bridge::{MessageTail, PaneRef};
use crate::remote_channel::transport::Message;
use crate::surface::strings as s;

const NOW: i64 = 1_800_000_000;

fn scope(connection: &str, bot: &str, conversation: &str) -> Scope {
    Scope {
        connection: connection.into(),
        bot: bot.into(),
        conversation: conversation.into(),
    }
}

fn work() -> Scope {
    scope("work", "bot", "D1")
}

fn key(agent: &str, session: &str) -> ConversationKey {
    ConversationKey {
        agent: agent.into(),
        session: session.into(),
    }
}

fn pane(id: u64) -> PaneRef {
    PaneRef {
        workspace: daruda_store::project::WorkspaceUuid::new(),
        pane: id,
    }
}

fn conversation(agent: &str, session: &str) -> PaneConversation {
    PaneConversation {
        key: key(agent, session),
        agent_name: format!("{agent} name"),
        title: "my_project\nClaude".into(),
    }
}

#[test]
fn a_thread_is_found_only_under_the_identity_that_made_it() {
    let mut book = Book::default();
    book.insert(&work(), key("claude", "s1"), "1.1".into(), NOW);
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
    book.insert(&work(), key("claude", "s1"), "1.1".into(), NOW);
    book.insert(&work(), key("claude", "s1"), "2.2".into(), NOW);
    assert_eq!(book.records().len(), 1, "the older claim is released");
    assert_eq!(book.records()[0].root, "2.2");
}

#[test]
fn a_new_conversation_continues_the_previous_thread() {
    let mut book = Book::default();
    book.insert(&work(), key("claude", "s1"), "1.1".into(), NOW);
    // Another thread already holds the conversation being continued into.
    book.insert(&work(), key("claude", "s2"), "9.9".into(), NOW);

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
    book.insert(&work(), key("claude", "0"), "1.1".into(), NOW);
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
    book.insert(&work(), key("claude", "s1"), "1.1".into(), NOW);
    assert!(!book.touch(&work(), "1.1", NOW + TOUCH_INTERVAL_SECS - 1));
    assert!(book.touch(&work(), "1.1", NOW + TOUCH_INTERVAL_SECS));
    assert!(!book.touch(&work(), "unknown", NOW + 10 * TOUCH_INTERVAL_SECS));
}

#[test]
fn pruning_drops_stale_threads_then_keeps_the_newest() {
    let mut book = Book::default();
    book.insert(
        &work(),
        key("claude", "old"),
        "0.0".into(),
        NOW - MAX_AGE_SECS - 1,
    );
    for n in 0..MAX_THREADS + 3 {
        book.insert(
            &work(),
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

/// One pane's messages over time, as the outbound queue places them.
#[test]
fn a_pane_keeps_its_thread_across_conversations() {
    let mut threads = Threads::load();
    let pane = pane(7);
    let claude = conversation("claude", "s1");

    assert_eq!(
        threads.place(&work(), pane, &claude, NOW),
        (None, None),
        "no thread yet"
    );
    threads.remember(&work(), claude.key.clone(), "1.1".into(), NOW);
    assert_eq!(
        threads.place(&work(), pane, &claude, NOW),
        (Some("1.1".into()), None),
        "the same conversation reuses its thread"
    );

    let cleared = conversation("claude", "s2");
    assert_eq!(
        threads.place(&work(), pane, &cleared, NOW),
        (
            Some("1.1".into()),
            Some(s::remote_channel::thread_new_conversation())
        ),
        "a new conversation of the same agent continues the thread"
    );

    let switched = conversation("codex", "s3");
    assert_eq!(
        threads.place(&work(), pane, &switched, NOW),
        (
            Some("1.1".into()),
            Some(s::remote_channel::thread_switched_agent("codex name"))
        ),
        "another agent continues the thread and is named"
    );

    assert_eq!(
        threads.place(&scope("work", "other", "D1"), pane, &switched, NOW),
        (None, None),
        "another bot starts its own thread"
    );
}

#[test]
fn a_reply_under_the_root_drops_the_header_unless_it_is_all_there_is() {
    let cases = [
        (MessageTail::Markdown("done".into()), "", "body present"),
        (
            MessageTail::Plain("  ".into()),
            "my_project",
            "blank body keeps the header",
        ),
    ];
    for (tail, header, label) in cases {
        let mut message = Message {
            header: "my_project".into(),
            tail,
            keyboard: None,
        };
        drop_header_under_root(&mut message);
        assert_eq!(message.header, header, "{label}");
    }
}
