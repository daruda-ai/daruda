//! Compact composer content lives only while its matching prompt is queued.

use std::collections::HashMap;

use crate::ui::InputContent;
use crate::workspace::main_area::pane_tree::PaneId;
use daruda_chat::view::{PromptId, PromptQueue};

#[derive(Default)]
pub(in crate::workspace) struct QueuedInputCache {
    panes: HashMap<PaneId, HashMap<PromptId, CachedInput>>,
}

struct CachedInput {
    prompt_text: String,
    content: InputContent,
}

impl QueuedInputCache {
    pub(super) fn content(
        &self,
        pane: PaneId,
        id: PromptId,
        prompt_text: &str,
    ) -> Option<&InputContent> {
        self.panes
            .get(&pane)?
            .get(&id)
            .filter(|input| input.prompt_text == prompt_text)
            .map(|input| &input.content)
    }

    pub(super) fn remember(
        &mut self,
        pane: PaneId,
        id: PromptId,
        prompt_text: &str,
        content: &InputContent,
    ) {
        if content.tokens().is_empty() {
            if let Some(inputs) = self.panes.get_mut(&pane) {
                inputs.remove(&id);
                if inputs.is_empty() {
                    self.panes.remove(&pane);
                }
            }
            return;
        }
        self.panes.entry(pane).or_default().insert(
            id,
            CachedInput {
                prompt_text: prompt_text.to_owned(),
                content: content.clone(),
            },
        );
    }

    pub(super) fn reconcile(&mut self, pane: PaneId, queue: &PromptQueue) {
        let Some(inputs) = self.panes.get_mut(&pane) else {
            return;
        };
        inputs.retain(|id, input| {
            queue
                .find(*id)
                .is_some_and(|q| q.text() == input.prompt_text)
        });
        if inputs.is_empty() {
            self.panes.remove(&pane);
        }
    }

    pub(super) fn has_pane(&self, pane: PaneId) -> bool {
        self.panes.contains_key(&pane)
    }

    pub(in crate::workspace) fn forget_pane(&mut self, pane: PaneId) {
        self.panes.remove(&pane);
    }
}

#[cfg(test)]
mod tests;
