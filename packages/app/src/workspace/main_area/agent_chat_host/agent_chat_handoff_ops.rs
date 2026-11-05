//! Continuing a pane's conversation in a fresh session under another account
//! or agent. Neither `session/load` nor a resume crosses accounts or agents,
//! so the pane keeps its transcript on screen, marks the switch, and briefs
//! the new session with a recap of what came before ([`daruda_acp::handoff`]).

use daruda_acp::handoff::{HandoffMark, HandoffParty, recap};
use daruda_chat::agent_chat_helpers::has_conversation;
use daruda_chat::view::AgentSessionStatus;
use daruda_store::accounts::AccountSelection;
use gpui::Context;

use super::agent_chat_ops::{agent_identity, agent_name_for};
use crate::surface::strings as s;
use crate::workspace::Workspace;
use crate::workspace::main_area::pane_menu::adapter::build_popup_menu;
use crate::workspace::main_area::pane_menu::spec::{
    Activate, ItemState, MenuEntry, disabled_item, item,
};
use crate::workspace::main_area::pane_tree::PaneId;
use daruda_chat::transcript_defaults::TranscriptDefaults;
use gpui::{Pixels, Point, Window};

/// What a session swap would put at risk in an agent chat pane — the one
/// reading every pane switch (account or agent) decides by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum ChatHold {
    /// A turn or a connect is under way: a teardown would cut live work.
    Busy,
    /// A conversation that must stay on its own session: a read-only mirror,
    /// or a task's chat, whose run is bound to that exact session.
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
        } else if self.task_chat_execution(pane_id, cx).is_some() {
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
        let mark = HandoffMark {
            from,
            to: self.handoff_party(pane_id, cx),
        };
        let recap = recap(view.read(cx).items(), &mark);
        let briefing = self.handoff_briefing(pane_id, recap, cx);
        self.restart_agent_chat_session(
            pane_id,
            cwd,
            move |v, cx| v.begin_handoff(mark, briefing, cx),
            cx,
        );
    }

    /// The header's agent menu: continue this conversation with another agent
    /// in the same pane, or open a new chat with one.
    pub(in crate::workspace) fn open_agent_menu(
        &mut self,
        pane_id: PaneId,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(hold), Some(view)) = (
            self.agent_chat_hold(pane_id, cx),
            self.agent_chat_view(pane_id),
        ) else {
            return;
        };
        let current = view.read(cx).agent_id().to_owned();
        let agents: Vec<(String, String)> = self
            .mirrors
            .agents
            .iter()
            .map(|a| (a.id.clone(), a.name.clone()))
            .collect();
        let entries = agent_menu_entries(pane_id, &agents, &current, hold);
        let menu = build_popup_menu(entries, None, cx.entity().downgrade(), window, cx);
        self.open_context_menu(position, menu, window, cx);
    }

    /// Point `pane_id` at `agent_id`: an empty pane simply reconnects under
    /// it, a conversation continues there from a recap.
    pub(in crate::workspace) fn switch_pane_agent(
        &mut self,
        pane_id: PaneId,
        agent_id: String,
        cx: &mut Context<Self>,
    ) {
        match self.agent_chat_hold(pane_id, cx) {
            Some(ChatHold::Conversation) => {
                self.continue_agent_chat_in_place(pane_id, HandoffTarget::Agent(agent_id), cx);
            }
            Some(ChatHold::Empty) => {
                self.set_agent_chat_account(pane_id, AccountSelection::SystemDefault);
                self.repoint_agent_chat(pane_id, agent_id, cx);
                self.reset_agent_chat_session(pane_id, cx);
            }
            // The menu disables the choice; a stale click lands here.
            Some(ChatHold::Busy | ChatHold::Pinned) | None => {}
        }
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

    /// Who is speaking in `pane_id` now: its agent, and its account's email
    /// when it is pinned to a managed one.
    fn handoff_party(&self, pane_id: PaneId, cx: &gpui::App) -> HandoffParty {
        let agent = self
            .agent_chat_view(pane_id)
            .map(|view| view.read(cx).agent_name().to_owned())
            .unwrap_or_default();
        let account = self
            .agent_chat_account_selection(pane_id)
            .account_id()
            .and_then(|id| self.accounts.find(id))
            .and_then(|account| account.email.clone());
        HandoffParty { agent, account }
    }

    /// What the new session hears before the next prompt; see
    /// [`compose_briefing`].
    fn handoff_briefing(
        &self,
        pane_id: PaneId,
        recap: Option<String>,
        cx: &gpui::App,
    ) -> Option<String> {
        let orchestrator = self
            .orchestrator
            .chat
            .as_ref()
            .is_some_and(|chat| chat.pane_id == pane_id)
            .then(|| crate::orchestrator::session_briefing(cx))
            .flatten();
        compose_briefing(recap, orchestrator)
    }
}

/// "Continue with ▸" every other agent — disabled, with the reason, while the
/// pane cannot hand off — then "New chat ▸" every agent.
fn agent_menu_entries(
    pane_id: PaneId,
    agents: &[(String, String)],
    current: &str,
    hold: ChatHold,
) -> Vec<MenuEntry> {
    let reason: Option<gpui::SharedString> = match hold {
        ChatHold::Busy => Some(s::agent_chat::continue_busy().into()),
        ChatHold::Pinned => Some(s::agent_chat::continue_pinned().into()),
        ChatHold::Conversation | ChatHold::Empty => None,
    };
    let continue_entries = agents
        .iter()
        .filter(|(id, _)| id != current)
        .map(|(id, name)| match &reason {
            Some(reason) => disabled_item(name.clone(), Some(reason.clone())),
            None => {
                let id = id.clone();
                item(
                    name.clone(),
                    ItemState::Enabled,
                    Activate::Op(Box::new(move |ws, _window, cx| {
                        ws.switch_pane_agent(pane_id, id.clone(), cx)
                    })),
                )
            }
        })
        .collect();
    let new_entries = agents
        .iter()
        .map(|(id, name)| {
            let id = id.clone();
            item(
                name.clone(),
                ItemState::Enabled,
                Activate::Op(Box::new(move |ws, window, cx| {
                    ws.open_agent_chat_pane_with_agent(id.clone(), window, cx)
                })),
            )
        })
        .collect();
    vec![
        MenuEntry::Submenu {
            label: s::agent_chat::continue_with().into(),
            entries: continue_entries,
        },
        MenuEntry::Submenu {
            label: s::common::new_agent_chat().into(),
            entries: new_entries,
        },
    ]
}

/// The recap, then — for the orchestrator's pane — the briefing its first
/// session had, since the fresh session has never heard it. The recap leads
/// so a replay still finds the block at the start of the prompt.
fn compose_briefing(recap: Option<String>, orchestrator: Option<String>) -> Option<String> {
    match (recap, orchestrator) {
        (Some(recap), Some(briefing)) => Some(format!("{recap}\n\n---\n\n{briefing}")),
        (recap, briefing) => recap.or(briefing),
    }
}

#[cfg(test)]
mod tests;
