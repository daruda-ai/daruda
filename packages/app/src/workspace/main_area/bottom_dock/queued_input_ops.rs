//! Preserve submitted input presentation while its matching prompt is queued.

use crate::ui::InputContent;
use crate::workspace::Workspace;
use crate::workspace::main_area::pane_tree::PaneId;
use daruda_chat::view::{AgentChatView, PromptId};
use gpui::{App, Context, Entity};

use super::QueuedInputCache;

pub(in crate::workspace) struct ComposerQueueSnapshot {
    pane: PaneId,
    ids: Vec<PromptId>,
    editing: Option<PromptId>,
}

impl Workspace {
    pub(in crate::workspace) fn queued_input_content(
        &self,
        pane: PaneId,
        id: PromptId,
        prompt_text: &str,
    ) -> InputContent {
        self.queued_input_cache(pane)
            .and_then(|cache| cache.content(pane, id, prompt_text))
            .cloned()
            .unwrap_or_else(|| InputContent::new(prompt_text.to_owned()))
    }

    pub(in crate::workspace) fn snapshot_composer_queue(
        &self,
        cx: &App,
    ) -> Option<ComposerQueueSnapshot> {
        let pane = self.bottom_input_pane()?;
        let queue = self.agent_chat_view(pane)?.read(cx).queue();
        Some(ComposerQueueSnapshot {
            pane,
            ids: queue.in_strip_order().map(|(q, _)| q.id()).collect(),
            editing: queue.editing(),
        })
    }

    pub(in crate::workspace) fn cache_queued_input(
        &mut self,
        before: Option<ComposerQueueSnapshot>,
        content: &InputContent,
        prompt_text: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(before) = before else {
            return;
        };
        let pane = before.pane;
        let Some(view) = self.agent_chat_view(pane).cloned() else {
            return;
        };
        let queue = view.read(cx).queue();
        let Some(cache) = self.ensure_queued_input_cache(pane) else {
            return;
        };
        cache.reconcile(pane, queue);
        // A newly queued item or the edited slot now contains the submitted
        // text. Other equal prompts keep their own presentation.
        let target = queue.in_strip_order().map(|(q, _)| q).find(|q| {
            (!before.ids.contains(&q.id()) || before.editing == Some(q.id()))
                && q.text() == prompt_text
        });
        if let Some(prompt) = target {
            cache.remember(pane, prompt.id(), prompt.text(), content);
        }
    }

    pub(in crate::workspace) fn prune_queued_input_cache(
        &mut self,
        pane: PaneId,
        view: &Entity<AgentChatView>,
        cx: &App,
    ) {
        let Some(cache) = self.queued_input_cache_mut(pane) else {
            return;
        };
        // Chat also notifies while streaming. Skip queue reads when this pane
        // has no cached input, and keep cache cleanup free of UI invalidation.
        if cache.has_pane(pane) {
            cache.reconcile(pane, view.read(cx).queue());
        }
    }

    fn queued_input_cache(&self, pane: PaneId) -> Option<&QueuedInputCache> {
        if self.is_orchestrator_pane(pane) {
            return self
                .orchestrator
                .chat
                .as_ref()
                .map(|chat| &chat.queued_input_cache);
        }
        let lane = self.lane_ref_for_pane(pane)?;
        self.lane_scoped
            .get(&lane)
            .map(|state| &state.queued_input_cache)
    }

    fn queued_input_cache_mut(&mut self, pane: PaneId) -> Option<&mut QueuedInputCache> {
        if self.is_orchestrator_pane(pane) {
            return self
                .orchestrator
                .chat
                .as_mut()
                .map(|chat| &mut chat.queued_input_cache);
        }
        let lane = self.lane_ref_for_pane(pane)?;
        self.lane_scoped
            .get_mut(&lane)
            .map(|state| &mut state.queued_input_cache)
    }

    // Notifications can arrive after lane teardown; only accepted submissions
    // may create lane-scoped cache state.
    fn ensure_queued_input_cache(&mut self, pane: PaneId) -> Option<&mut QueuedInputCache> {
        if self.is_orchestrator_pane(pane) {
            return self
                .orchestrator
                .chat
                .as_mut()
                .map(|chat| &mut chat.queued_input_cache);
        }
        let lane = self.lane_ref_for_pane(pane)?;
        Some(&mut self.lane_scoped_mut(lane).queued_input_cache)
    }
}

#[cfg(test)]
mod tests {
    // Queue round trips are covered with real composer submission in file_context::tests.
}
