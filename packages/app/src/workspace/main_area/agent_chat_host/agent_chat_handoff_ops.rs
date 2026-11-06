//! Continuing a pane's conversation in a fresh session under another account
//! or agent. Neither `session/load` nor a resume crosses accounts or agents,
//! so the pane keeps its transcript on screen, marks the switch, and briefs
//! the new session with a recap of what came before ([`daruda_acp::handoff`]).

use daruda_acp::handoff::HandoffParty;
use daruda_chat::agent_chat_helpers::has_conversation;
use daruda_chat::view::AgentSessionStatus;
use daruda_store::accounts::AccountSelection;
use gpui::Context;

use super::agent_chat_ops::{agent_identity, agent_name_for};
use crate::surface::strings as s;
use crate::workspace::Workspace;
use crate::workspace::main_area::pane_tree::PaneId;
use daruda_chat::transcript_defaults::TranscriptDefaults;

/// What a session swap would put at risk in an agent chat pane — the one
/// reading every pane switch (account or agent) decides by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum ChatHold {
    /// A turn or a connect is under way: a teardown would cut live work.
    Busy,
    /// A conversation that must stay on its own session: a read-only mirror,
    /// or a pane bound to a task execution, whose delivery a teardown would
    /// fail.
    Pinned,
    /// A conversation a fresh session can continue from a recap.
    Conversation,
    /// Nothing to carry over.
    Empty,
}

/// Where a pane's conversation moves.
pub(in crate::workspace) enum HandoffTarget {
    Account(AccountSelection),
    Agent(String),
}

impl Workspace {
    /// [`ChatHold`] for an agent chat pane; `None` for any other pane.
    ///
    /// Busy is `is_busy` (the single activity-state source) plus a connect
    /// handshake already in flight: that is connection lifecycle rather than a
    /// turn, so `is_busy` doesn't cover it, but a teardown under one would
    /// race two connect attempts.
    pub(in crate::workspace) fn agent_chat_hold(
        &self,
        pane_id: PaneId,
        cx: &gpui::App,
    ) -> Option<ChatHold> {
        let view = self.agent_chat_view(pane_id)?.read(cx);
        let busy = view.is_busy()
            || matches!(
                view.status(),
                AgentSessionStatus::PreparingRuntime(_)
                    | AgentSessionStatus::Connecting
                    | AgentSessionStatus::Handshaking(_)
            );
        Some(if busy {
            ChatHold::Busy
        } else if view.is_read_only() {
            ChatHold::Pinned
        } else if !has_conversation(view.items()) {
            ChatHold::Empty
        } else if crate::runs::RunsHost::pane_execution(self.run_pane(pane_id), cx).is_some() {
            // The same binding `restart_agent_chat_session` releases as torn
            // down — not the narrower "still the task's session" match.
            ChatHold::Pinned
        } else {
            ChatHold::Conversation
        })
    }

    /// Move `pane_id`'s conversation to `target` in the same pane: re-point
    /// the pane, mark the switch, and start a fresh session briefed with a
    /// recap. The caller has decided the pane holds a [`ChatHold::Conversation`].
    pub(in crate::workspace) fn continue_agent_chat_in_place(
        &mut self,
        pane_id: PaneId,
        target: HandoffTarget,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.agent_chat_view(pane_id).cloned() else {
            return;
        };
        let Some(cwd) = view.read(cx).cwd().cloned() else {
            return;
        };
        let from = self.handoff_party(pane_id, cx);
        match target {
            HandoffTarget::Account(selection) => self.set_agent_chat_account(pane_id, selection),
            HandoffTarget::Agent(agent_id) => {
                // A pinned account belongs to the old agent's sign-in domain.
                self.set_agent_chat_account(pane_id, AccountSelection::SystemDefault);
                self.repoint_agent_chat(pane_id, agent_id, cx);
            }
        }
        let to = self.handoff_party(pane_id, cx);
        let briefing = self.orchestrator_briefing(pane_id, cx);
        self.restart_agent_chat_session(
            pane_id,
            cwd,
            move |v, cx| {
                v.begin_handoff(from, to, cx);
                // The fresh session has never heard the orchestrator's briefing.
                if let Some(briefing) = briefing {
                    v.set_briefing(briefing);
                }
            },
            cx,
        );
    }

    /// Point a live pane at `agent_id`, keeping the view, both wrapper caches
    /// and the agent's question timeout in step.
    pub(in crate::workspace) fn repoint_agent_chat(
        &mut self,
        pane_id: PaneId,
        agent_id: String,
        cx: &mut Context<Self>,
    ) {
        let defaults = TranscriptDefaults::resolve(
            self.mirrors.agents.iter().find(|a| a.id == agent_id),
            self.mirrors.agent_reader_defaults,
        );
        if let Some(view) = self.agent_chat_view(pane_id).cloned() {
            let name = agent_name_for(&self.mirrors.agents, &agent_id);
            let question_timeout = self.question_timeout_for(&agent_id);
            let identity = agent_identity(agent_id.clone(), name);
            view.update(cx, |v, cx| {
                v.switch_agent(identity, &defaults, cx);
                v.set_question_timeout(question_timeout, cx);
            });
        }
        self.update_agent_chat_agent_id(pane_id, agent_id);
    }

    /// Who is speaking in `pane_id` now: its agent, and — when it is pinned
    /// to a managed account — that account's email, or a stand-in when none
    /// was captured, so an account switch never reads as no change.
    fn handoff_party(&self, pane_id: PaneId, cx: &gpui::App) -> HandoffParty {
        let agent = self
            .agent_chat_view(pane_id)
            .map(|view| view.read(cx).agent_name().to_owned())
            .unwrap_or_default();
        let account = self
            .agent_chat_account_selection(pane_id)
            .account_id()
            .map(|id| {
                self.accounts
                    .find(id)
                    .and_then(|account| account.email.clone())
                    .unwrap_or_else(s::settings::accounts_unknown_email)
            });
        HandoffParty { agent, account }
    }

    /// The briefing the orchestrator's pane starts every session with; `None`
    /// for any other pane.
    fn orchestrator_briefing(&self, pane_id: PaneId, cx: &gpui::App) -> Option<String> {
        self.orchestrator
            .chat
            .as_ref()
            .is_some_and(|chat| chat.pane_id == pane_id)
            .then(|| crate::orchestrator::session_briefing(cx))
            .flatten()
    }
}
