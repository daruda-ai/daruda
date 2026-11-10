//! What a remote pane thread needs to know about a pane.

use crate::remote_channel::threads::{ConversationKey, PaneConversation};
use crate::workspace::Workspace;
use crate::workspace::main_area::pane_tree::PaneId;
use gpui::Context;

impl Workspace {
    /// The pane's live conversation and the title its thread root shows.
    /// `None` until the agent has established a session.
    pub(crate) fn remote_conversation(
        &self,
        pane_id: PaneId,
        cx: &Context<Self>,
    ) -> Option<PaneConversation> {
        let view = self.agent_chat_view(pane_id)?.read(cx);
        Some(PaneConversation {
            key: ConversationKey {
                agent: view.agent_id().to_owned(),
                session: view.session_id()?.to_owned(),
            },
            agent_name: view.agent_name().to_owned(),
            title: self.remote_header(pane_id, cx),
        })
    }
}
