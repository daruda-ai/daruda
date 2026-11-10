//! Pane thread tests, one file per part: the thread table, placing a post,
//! the root's status, and routing replies.

use super::plan::{Placement, ThreadPlan};
use super::state::Threads;
use super::*;
use crate::remote_channel::bridge::{PaneRef, PingWeight};

mod book;
mod place;
mod roots;
mod route;

pub(super) const NOW: i64 = 1_800_000_000;

pub(super) fn scope(connection: &str, bot: &str, conversation: &str) -> Scope {
    Scope {
        connection: connection.into(),
        bot: bot.into(),
        conversation: conversation.into(),
    }
}

pub(super) fn work() -> Scope {
    scope("work", "bot", "D1")
}

pub(super) fn key(agent: &str, session: &str) -> ConversationKey {
    ConversationKey {
        agent: agent.into(),
        session: session.into(),
    }
}

pub(super) fn pane(id: u64) -> PaneRef {
    PaneRef {
        workspace: daruda_store::project::WorkspaceUuid::new(),
        pane: id,
    }
}

pub(super) fn conversation(agent: &str, session: &str) -> PaneConversation {
    PaneConversation {
        key: key(agent, session),
        agent_name: format!("{agent} name"),
        name: RootName {
            title: "my_project · Claude".into(),
            subject: None,
        },
    }
}

/// Plan a post by `pane` the way the outbound queue does.
pub(super) fn plan(
    threads: &mut Threads,
    scope: &Scope,
    pane: PaneRef,
    conversation: &PaneConversation,
    weight: PingWeight,
) -> ThreadPlan {
    threads.plan_post(scope.clone(), pane, conversation.clone(), weight, NOW)
}

/// A first post by `pane`: it starts a thread rooted at `root`.
pub(super) fn start_thread(
    threads: &mut Threads,
    scope: &Scope,
    pane: PaneRef,
    conversation: &PaneConversation,
    root: &str,
) {
    let first = plan(threads, scope, pane, conversation, PingWeight::Progress);
    assert_eq!(first.placement, Placement::Start);
    threads.root_started(&first, root.into(), NOW);
}
