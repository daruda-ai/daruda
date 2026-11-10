//! What a remote pane thread needs to know about a pane.

use crate::remote_channel::bridge::PaneRef;
use crate::remote_channel::threads::status::{Ending, PaneActivity};
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

    /// Tell the pane's remote threads what it is doing now. Called on each
    /// pulse tick of a busy pane; an unchanged status costs one lookup.
    pub(in crate::workspace) fn report_remote_busy(&self, pane_id: PaneId, cx: &mut Context<Self>) {
        let Some(view) = self.agent_chat_view(pane_id) else {
            return;
        };
        let view = view.read(cx);
        if !view.last_reconciled_busy() {
            return;
        }
        let activity = match view.activity_state() {
            daruda_chat::view::ActivityState::AwaitingPermission
            | daruda_chat::view::ActivityState::AwaitingAnswer => PaneActivity::AwaitingYou,
            daruda_chat::view::ActivityState::Working | daruda_chat::view::ActivityState::Idle => {
                PaneActivity::Working
            }
        };
        self.report_remote_activity(pane_id, activity, cx);
    }

    /// Tell the pane's remote threads how its turn ended.
    pub(in crate::workspace) fn report_remote_ending(
        &self,
        pane_id: PaneId,
        ending: Ending,
        cx: &mut Context<Self>,
    ) {
        self.report_remote_activity(pane_id, PaneActivity::Ended(ending), cx);
    }

    fn report_remote_activity(
        &self,
        pane_id: PaneId,
        activity: PaneActivity,
        cx: &mut Context<Self>,
    ) {
        let pane = PaneRef {
            workspace: self.uuid(),
            pane: pane_id,
        };
        crate::remote_channel::threads::roots::pane_activity(pane, activity, cx);
    }
}
