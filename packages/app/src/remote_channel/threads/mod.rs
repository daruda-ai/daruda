//! One thread per pane on channels that support it: a root naming the pane,
//! the pane's messages under it, and the table that maps a root back to the
//! pane's conversation across restarts.

mod book;
pub(crate) mod driver;
mod look;
mod plan;
mod root_queue;
pub(crate) mod roots;
pub(crate) mod route;
pub(crate) mod state;
pub(crate) mod status;
pub(crate) mod stop;

use book::Book;
pub(crate) use book::Scope;
pub(crate) use daruda_store::remote_threads::ConversationKey;

/// What a pane's thread needs to know about the pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PaneConversation {
    pub key: ConversationKey,
    pub agent_name: String,
    pub name: RootName,
}

/// What a pane's root calls it: the pane on one line, and what its
/// conversation is about, as the pane's own activity bar says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RootName {
    pub title: String,
    pub subject: Option<String>,
}

#[cfg(test)]
mod tests;

/// Unix seconds now.
pub(crate) fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
}
