//! Session history keeps the composer's compact presentation beside wire text.

use std::collections::HashMap;

use crate::lane::history::HistoryBuffer;
use crate::ui::InputContent;

#[derive(Default)]
pub(in crate::workspace) struct InputHistory {
    buffer: HistoryBuffer,
    references: HashMap<String, InputContent>,
    draft: Option<InputContent>,
}

impl InputHistory {
    pub(in crate::workspace) fn push(&mut self, text: &str) {
        self.buffer.push(text);
        self.references.remove(text);
        self.draft = None;
    }

    pub(super) fn push_content(&mut self, text: &str, content: &InputContent) {
        self.push(text);
        if !content.tokens().is_empty() {
            self.references.insert(text.to_owned(), content.clone());
        }
    }

    pub(in crate::workspace) fn has_entries(&self) -> bool {
        self.buffer.has_entries()
    }

    pub(in crate::workspace) fn is_navigating(&self) -> bool {
        self.buffer.is_navigating()
    }

    pub(in crate::workspace) fn prev(
        &mut self,
        current: impl Into<InputContent>,
    ) -> Option<InputContent> {
        let current = current.into();
        if !self.is_navigating() {
            self.draft = Some(current.clone());
        }
        let text = self.buffer.prev(current.text())?;
        Some(
            self.references
                .get(text)
                .cloned()
                .unwrap_or_else(|| InputContent::new(text.to_owned())),
        )
    }

    pub(in crate::workspace) fn forward(&mut self) -> Option<InputContent> {
        let text = self.buffer.forward()?.to_owned();
        if !self.buffer.is_navigating() {
            return Some(self.draft.take().unwrap_or_else(|| InputContent::new(text)));
        }
        Some(
            self.references
                .get(&text)
                .cloned()
                .unwrap_or_else(|| InputContent::new(text)),
        )
    }
}

#[cfg(test)]
mod tests;
