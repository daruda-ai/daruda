//! Pane thread tests, one file per part: the thread table, placing a post,
//! the root's status, and routing replies.

use super::*;
use crate::remote_channel::bridge::PaneRef;

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
        title: "my_project\nClaude".into(),
    }
}
