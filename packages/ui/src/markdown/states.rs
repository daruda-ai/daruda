//! Parsed Markdown documents a host keeps across frames.
//!
//! Without a store, a [`super::Markdown`] keeps its parse in GPUI keyed state,
//! which lives only while the element is drawn in consecutive frames. A host
//! that virtualizes its rows stops drawing a row the moment it scrolls past the
//! overdraw, so the parse is dropped and redone when the row returns. A body
//! over the `TextView` synchronous-parse limit then lays out empty until the
//! background parse lands, and the row's height collapses under the reader.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{App, AppContext as _, ElementId, Entity, SharedString};
use gpui_base::TextViewState;

/// Parsed documents by element id, owned by the host that draws them.
///
/// Filled from render: an entry is created the first time an id is drawn, the
/// same moment keyed state would create it, so only bodies that were actually
/// shown are parsed. The host empties it when the ids stop naming the same
/// content, such as a conversation reset. Unlike keyed state, an id is not
/// scoped by its element path, so it must be unique across the host's bodies:
/// two bodies sharing one would share, and keep replacing, one document.
#[derive(Clone, Default)]
pub struct MarkdownStates(Rc<RefCell<HashMap<ElementId, Entity<TextViewState>>>>);

impl MarkdownStates {
    /// Drop every retained document.
    pub fn clear(&self) {
        self.0.borrow_mut().clear();
    }

    /// The document for `id`, holding `text`. A text that extends the previous
    /// one streams in as an append; any other text replaces it.
    pub(super) fn state_for(
        &self,
        id: &ElementId,
        text: &SharedString,
        cx: &mut App,
    ) -> Entity<TextViewState> {
        let existing = self.0.borrow().get(id).cloned();
        let state = match existing {
            Some(state) => state,
            None => {
                let state = cx.new(|cx| TextViewState::markdown(text, cx));
                self.0.borrow_mut().insert(id.clone(), state.clone());
                state
            }
        };
        state.update(cx, |state, cx| state.set_text(text, cx));
        state
    }
}

#[cfg(test)]
#[path = "states_tests.rs"]
mod tests;
