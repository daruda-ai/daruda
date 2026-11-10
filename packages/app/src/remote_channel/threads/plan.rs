//! What one pane message's post was decided to be: which thread, as what.

use super::status::ShownStatus;
use super::{ConversationKey, RootName, Scope};
use crate::remote_channel::bridge::PaneRef;

/// Where one pane message goes.
pub(crate) struct ThreadPlan {
    pub(super) scope: Scope,
    /// The thread's id: its record's, or the one a root started now takes.
    pub(super) id: String,
    pub(super) pane: PaneRef,
    pub(super) key: ConversationKey,
    pub(super) name: RootName,
    /// What a root started by this post shows.
    pub(super) shown: Option<ShownStatus>,
    pub(super) placement: Placement,
}

/// Whether the post starts the pane's thread or replies in it.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Placement {
    /// The pane has no thread on this connection yet: the post starts one.
    Start,
    Reply {
        root: String,
        /// Said in the thread first: the pane moved to a new conversation.
        divider: Option<String>,
        /// Also shown in the conversation, where the root has scrolled away.
        broadcast: bool,
    },
}
