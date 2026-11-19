//! The pull request details open in a window, found by the id each was
//! opened with. One detail is one pull request's reads, drafts and actions
//! under way; the page shows one of them, and a worktree's pull request
//! pane holds one. Where both are on the same pull request they are on the
//! same detail, so nothing is read twice and nothing typed is lost between
//! them.
//!
//! A detail lives while something holds it. Holding is ownership, not a
//! count kept by hand: whatever shows a detail owns a [`DetailHold`], and
//! the detail is over when the last hold is dropped, however its owner
//! went away. No path that removes a pane has to remember to let go.

use std::rc::Rc;

use daruda_github::RepoId;

use super::detail::{PrDetailId, PrDetailState};

/// A claim on an open detail. While one exists the detail stays open;
/// dropping the last ends it.
#[derive(Clone)]
pub(in crate::workspace) struct DetailHold {
    id: PrDetailId,
    /// Shared with the detail's entry, whose count of it is the holds
    /// alive. Never read: it is here to be dropped.
    _held: Rc<()>,
}

impl DetailHold {
    pub(in crate::workspace) fn id(&self) -> PrDetailId {
        self.id
    }
}

struct Open {
    state: PrDetailState,
    /// The entry's own share; any count above one is a hold alive.
    held: Rc<()>,
}

impl Open {
    fn is_held(&self) -> bool {
        Rc::strong_count(&self.held) > 1
    }
}

#[derive(Default)]
pub(in crate::workspace) struct PrDetails {
    /// Every detail still kept. One no longer held is not found by anyone
    /// and waits only for [`Self::take_ended`].
    open: Vec<Open>,
    /// The page's hold on the detail it shows over its list.
    page: Option<DetailHold>,
}

impl PrDetails {
    fn held(&self) -> impl Iterator<Item = &Open> {
        self.open.iter().filter(|open| open.is_held())
    }

    pub(in crate::workspace) fn get(&self, id: PrDetailId) -> Option<&PrDetailState> {
        self.held()
            .find(|open| open.state.id() == id)
            .map(|open| &open.state)
    }

    pub(in crate::workspace) fn get_mut(&mut self, id: PrDetailId) -> Option<&mut PrDetailState> {
        self.open
            .iter_mut()
            .filter(|open| open.is_held())
            .find(|open| open.state.id() == id)
            .map(|open| &mut open.state)
    }

    /// The detail the page shows.
    pub(in crate::workspace) fn on_page(&self) -> Option<&PrDetailState> {
        self.get(self.page.as_ref()?.id())
    }

    /// For a capture that seeds the shown detail.
    #[cfg(feature = "screenshot")]
    pub(in crate::workspace) fn on_page_mut(&mut self) -> Option<&mut PrDetailState> {
        let id = self.page.as_ref()?.id();
        self.get_mut(id)
    }

    /// The open detail of pull request `number` of `repo`, if there is one.
    pub(in crate::workspace) fn of(&self, repo: &RepoId, number: u64) -> Option<PrDetailId> {
        self.held()
            .map(|open| &open.state)
            .find(|detail| detail.item().repo == *repo && detail.item().pr.number == number)
            .map(PrDetailState::id)
    }

    /// Keep `detail` open for as long as the hold returned, or one taken
    /// from it, is alive.
    pub(in crate::workspace) fn open(&mut self, detail: PrDetailState) -> DetailHold {
        let held = Rc::new(());
        let hold = DetailHold {
            id: detail.id(),
            _held: held.clone(),
        };
        self.open.push(Open {
            state: detail,
            held,
        });
        hold
    }

    /// Another hold on the open detail `id`. `None` when it is not open.
    pub(in crate::workspace) fn hold(&self, id: PrDetailId) -> Option<DetailHold> {
        self.held()
            .find(|open| open.state.id() == id)
            .map(|open| DetailHold {
                id,
                _held: open.held.clone(),
            })
    }

    /// Show on the page the detail `next` holds, or nothing. The page lets
    /// go of the one it showed.
    pub(in crate::workspace) fn show_on_page(&mut self, next: Option<DetailHold>) {
        self.page = next;
    }

    /// Take out the details nothing holds any more, for the caller to
    /// settle what each leaves behind.
    pub(in crate::workspace) fn take_ended(&mut self) -> Vec<PrDetailState> {
        let (kept, ended): (Vec<Open>, Vec<Open>) = std::mem::take(&mut self.open)
            .into_iter()
            .partition(Open::is_held);
        self.open = kept;
        ended.into_iter().map(|open| open.state).collect()
    }

    /// How many details are open, held by something.
    #[cfg(test)]
    pub(in crate::workspace) fn len(&self) -> usize {
        self.held().count()
    }
}

#[cfg(test)]
mod tests;
