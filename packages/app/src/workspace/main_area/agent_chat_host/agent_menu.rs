//! The chat header's agent menu: continue this conversation with another
//! agent in the same pane, or open a new chat with one.

use gpui::{Context, Pixels, Point, Window};

use super::agent_chat_handoff_ops::{ChatHold, HandoffTarget};
use crate::surface::strings as s;
use crate::workspace::Workspace;
use crate::workspace::main_area::pane_menu::adapter::build_popup_menu;
use crate::workspace::main_area::pane_menu::spec::{
    Activate, ItemState, MenuEntry, disabled_item, item, normalize_entries,
};
use crate::workspace::main_area::pane_tree::PaneId;
use daruda_store::accounts::AccountSelection;

impl Workspace {
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
    // A one-agent catalog has nobody to continue with; that submenu drops.
    normalize_entries(vec![
        MenuEntry::Submenu {
            label: s::agent_chat::continue_with().into(),
            entries: continue_entries,
        },
        MenuEntry::Submenu {
            label: s::common::new_agent_chat().into(),
            entries: new_entries,
        },
    ])
}

#[cfg(test)]
mod tests;
