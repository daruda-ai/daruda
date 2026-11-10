//! One thread per pane on channels that support it: a root naming the pane,
//! the pane's messages under it, and the table that maps a root back to the
//! pane's conversation across restarts.

mod book;
pub(crate) mod driver;
pub(crate) mod roots;
pub(crate) mod status;
pub(crate) mod stop;

pub(crate) use book::{Book, Scope};
pub(crate) use daruda_store::remote_threads::ConversationKey;

/// What a pane's thread needs to know about the pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PaneConversation {
    pub key: ConversationKey,
    pub agent_name: String,
    /// The root's text: the same header the pane's messages carry.
    pub title: String,
}

#[cfg(test)]
mod tests;
