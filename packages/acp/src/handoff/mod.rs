//! Carrying a conversation across a session swap in the same pane.
//!
//! A pane that moves to another account or agent starts a fresh ACP session:
//! neither `session/load` nor a resume crosses accounts or agents. The new
//! session learns what came before from a recap — a plain-text record of the
//! pane's transcript ([`recap`]), wrapped in a `<conversation_handoff>` block
//! ([`block`]) and sent once in front of the next prompt. The block's opening
//! tag also carries the [`HandoffMark`], so a `session/load` replay of that
//! prompt folds back into the transcript divider instead of a bubble the
//! person never typed — and the record it carried comes back with it,
//! standing in for the conversation the replay of this session cannot restore.
//!
//! The block outlives this build: the agent stores the prompt in its own
//! session history, and replays it on every later load. Its format changes
//! only additively.

mod block;
mod recap;

pub use block::split_handoff;

use crate::model::ChatItem;

/// One side of a handoff: the agent and, when the pane names one, the account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffParty {
    pub agent: String,
    pub account: Option<String>,
}

/// Where a pane's conversation moved from and to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffMark {
    pub from: HandoffParty,
    pub to: HandoffParty,
}

impl HandoffMark {
    /// Whether the conversation leaves one agent for another — and with it,
    /// usually, one provider for another.
    fn crosses_agents(&self) -> bool {
        self.from.agent != self.to.agent
    }
}

/// The transcript's switch divider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffItem {
    pub mark: HandoffMark,
    /// What the recap said about the conversation before the switch, when that
    /// conversation is no longer on screen — `Some` only for a divider rebuilt
    /// from a recap block (a `session/load` replay, a restored pane). Live, the
    /// conversation itself is still above the divider.
    pub record: Option<String>,
}

impl HandoffItem {
    /// The divider a switch pushes while its conversation is on screen.
    pub fn live(mark: HandoffMark) -> Self {
        Self { mark, record: None }
    }
}

/// A switch, ready to apply: the divider to push and the recap block to send
/// ahead of the next prompt (`None` when there is nothing to carry over).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handoff {
    pub divider: HandoffItem,
    pub recap: Option<String>,
}

/// Prepare moving `items` from `from` to `to`.
///
/// A divider still trailing the transcript is a switch nobody spoke after, so
/// this one replaces it and keeps its origin — and its record, when a restart
/// rebuilt it: that record is all that is left of the conversation before it.
/// When the conversation crosses to another agent, the mark carries no
/// account addresses — the block goes to that agent's provider, and they are
/// not its business. The recap body never names accounts.
pub fn prepare(items: &[ChatItem], from: HandoffParty, to: HandoffParty) -> Handoff {
    let (items, from, carried) = match items.split_last() {
        Some((ChatItem::Handoff(pending), earlier)) => {
            (earlier, pending.mark.from.clone(), pending.record.clone())
        }
        _ => (items, from, None),
    };
    let mut mark = HandoffMark { from, to };
    if mark.crosses_agents() {
        mark.from.account = None;
        mark.to.account = None;
    }
    let recap = recap::transcript(items, carried.as_deref(), &mark)
        .map(|transcript| block::wrap(&mark, &recap::preamble(&mark), &transcript));
    Handoff {
        divider: HandoffItem {
            mark,
            record: carried,
        },
        recap,
    }
}
