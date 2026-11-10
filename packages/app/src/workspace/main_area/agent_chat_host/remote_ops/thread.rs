//! What a remote pane thread needs to know about a pane.

use crate::remote_channel::threads::{ConversationKey, PaneConversation};
use crate::workspace::Workspace;
use crate::workspace::main_area::pane_tree::PaneId;
use gpui::{App, Context};

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

    /// The open pane holding one of `conversations`: where a reply in that
    /// conversation's thread goes.
    pub(crate) fn pane_with_conversation(
        &self,
        conversations: &[ConversationKey],
        cx: &App,
    ) -> Option<PaneId> {
        self.every_agent_chat()
            .find(|(_, view)| {
                let view = view.read(cx);
                view.session_id().is_some_and(|session| {
                    conversations
                        .iter()
                        .any(|key| key.agent == view.agent_id() && key.session == session)
                })
            })
            .map(|(pane, _)| pane)
    }
}
