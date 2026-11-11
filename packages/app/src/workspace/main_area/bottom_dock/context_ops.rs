//! Compact queue drafts are a presentation cache, validated against queue text.

use crate::ui::InputContent;
use crate::workspace::Workspace;
use crate::workspace::main_area::pane_tree::PaneId;
use daruda_chat::view::PromptId;
use gpui::{App, Context};

pub(in crate::workspace) struct QueuedDraft {
    wire: String,
    content: InputContent,
}

pub(in crate::workspace) struct ComposerQueueState {
    pane: PaneId,
    ids: Vec<PromptId>,
    editing: Option<PromptId>,
}

impl Workspace {
    pub(in crate::workspace) fn queued_composer_content(
        &self,
        pane: PaneId,
        id: PromptId,
        wire: &str,
    ) -> InputContent {
        self.lane_scoped
            .get(&self.active_ref())
            .and_then(|state| state.queued_drafts.get(&(pane, id)))
            .filter(|draft| draft.wire == wire)
            .map(|draft| draft.content.clone())
            .unwrap_or_else(|| InputContent::new(wire.to_owned()))
    }

    pub(in crate::workspace) fn composer_queue_state(
        &self,
        cx: &App,
    ) -> Option<ComposerQueueState> {
        let pane = self.bottom_input_pane()?;
        let queue = self.agent_chat_view(pane)?.read(cx).queue();
        Some(ComposerQueueState {
            pane,
            ids: queue.in_strip_order().map(|(q, _)| q.id()).collect(),
            editing: queue.editing(),
        })
    }

    pub(in crate::workspace) fn remember_queued_composer(
        &mut self,
        before: Option<ComposerQueueState>,
        content: &InputContent,
        wire: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(before) = before else {
            return;
        };
        let pane = before.pane;
        let Some(view) = self.agent_chat_view(pane) else {
            return;
        };
        let queued: Vec<_> = view
            .read(cx)
            .queue()
            .in_strip_order()
            .map(|(q, _)| (q.id(), q.text().to_owned()))
            .collect();
        let cache = &mut self.lane_scoped_mut(self.active_ref()).queued_drafts;
        cache.retain(|(owner, id), saved| {
            *owner != pane
                || queued
                    .iter()
                    .any(|(live, text)| live == id && *text == saved.wire)
        });
        // A newly queued item or the edited slot now contains the submitted
        // wire text. Other equal prompts keep their own presentation.
        let target = queued.iter().find(|(id, text)| {
            (!before.ids.contains(id) || before.editing == Some(*id)) && text == wire
        });
        if let Some((id, text)) = target {
            if content.tokens().is_empty() {
                cache.remove(&(pane, *id));
            } else {
                cache.insert(
                    (pane, *id),
                    QueuedDraft {
                        wire: text.clone(),
                        content: content.clone(),
                    },
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    // Queue round trips are covered with real composer submission in file_context::tests.
}
