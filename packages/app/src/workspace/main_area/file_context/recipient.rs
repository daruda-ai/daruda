//! Recipient presentation shared by the file viewer's send actions.

use crate::{surface::strings as s, ui, workspace::Workspace};
use gpui::SharedString;

#[derive(Clone)]
pub(in crate::workspace) struct FileSendAction {
    pub label: SharedString,
    pub tooltip: SharedString,
    pub icon: ui::Icon,
}

impl Workspace {
    pub(in crate::workspace) fn file_send_action(&self) -> FileSendAction {
        let recipient = self.bottom_input_pane().and_then(|id| {
            self.active_runtime()
                .panes
                .iter()
                .find(|pane| pane.id == id)
        });
        let identity = recipient.and_then(|pane| match &pane.content {
            super::super::pane::PaneContent::AgentChat(chat) => {
                let name = self
                    .mirrors
                    .agents
                    .iter()
                    .find(|agent| agent.id == chat.agent_id)
                    .map(|agent| agent.name.trim())
                    .filter(|name| !name.is_empty())
                    .unwrap_or(&chat.agent_id);
                Some((
                    name.to_string(),
                    daruda_config::agent::icon::icon_for_agent(&chat.agent_id),
                ))
            }
            super::super::pane::PaneContent::Terminal(terminal) => {
                Some((terminal.cached_title.to_string(), Some(ui::icons::TERMINAL)))
            }
            super::super::pane::PaneContent::File(_) => None,
        });
        match identity {
            Some((name, icon)) => FileSendAction {
                label: s::file_viewer::send_to(&name).into(),
                tooltip: s::file_viewer::send_to_input_hint(&name).into(),
                icon: ui::agent_menu_icon(icon),
            },
            None => FileSendAction {
                label: s::file_viewer::send_to_agent().into(),
                tooltip: s::file_viewer::input_unavailable().into(),
                icon: ui::agent_menu_icon(None),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    // Recipient switching and source actions are covered by the parent integration tests.
}
