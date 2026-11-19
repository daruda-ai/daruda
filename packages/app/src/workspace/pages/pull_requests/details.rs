//! The pull request details open in a window, found by the id each was
//! opened with. One detail is one pull request's reads, drafts and actions
//! under way; the page shows one of them, and a worktree's pull request
//! pane holds one. Where both are on the same pull request they are on the
//! same detail, so nothing is read twice and nothing typed is lost between
//! them.
//!
//! A detail lives while something shows it: the page, or at least one pane.

use daruda_github::RepoId;

use super::detail::{PrDetailId, PrDetailState};

#[derive(Default)]
pub(in crate::workspace) struct PrDetails {
    open: Vec<PrDetailState>,
    /// The one the page shows over its list.
    page: Option<PrDetailId>,
    /// One entry per pane holding a detail; a detail held twice is here
    /// twice.
    panes: Vec<PrDetailId>,
}

impl PrDetails {
    pub(in crate::workspace) fn get(&self, id: PrDetailId) -> Option<&PrDetailState> {
        self.open.iter().find(|detail| detail.id() == id)
    }

    pub(in crate::workspace) fn get_mut(&mut self, id: PrDetailId) -> Option<&mut PrDetailState> {
        self.open.iter_mut().find(|detail| detail.id() == id)
    }

    /// The detail the page shows.
    pub(in crate::workspace) fn on_page(&self) -> Option<&PrDetailState> {
        self.get(self.page?)
    }

    #[cfg(feature = "screenshot")]
    pub(in crate::workspace) fn on_page_mut(&mut self) -> Option<&mut PrDetailState> {
        self.get_mut(self.page?)
    }

    /// The open detail of pull request `number` of `repo`, if there is one.
    pub(in crate::workspace) fn of(&self, repo: &RepoId, number: u64) -> Option<PrDetailId> {
        self.open
            .iter()
            .find(|detail| detail.item().repo == *repo && detail.item().pr.number == number)
            .map(PrDetailState::id)
    }

    /// Keep `detail` open. It stays only while something shows it: put it
    /// on the page or give it to a pane next.
    pub(in crate::workspace) fn open(&mut self, detail: PrDetailState) -> PrDetailId {
        let id = detail.id();
        self.open.push(detail);
        id
    }

    /// Show `next` on the page, or nothing. The detail the page showed
    /// before is returned when this ends it: no pane holds it either.
    pub(in crate::workspace) fn show_on_page(
        &mut self,
        next: Option<PrDetailId>,
    ) -> Option<PrDetailState> {
        let next = next.filter(|id| self.get(*id).is_some());
        let before = std::mem::replace(&mut self.page, next);
        before
            .filter(|id| self.page != Some(*id))
            .and_then(|id| self.end_if_unshown(id))
    }

    /// A pane now shows `id`.
    pub(in crate::workspace) fn hold(&mut self, id: PrDetailId) {
        if self.get(id).is_some() {
            self.panes.push(id);
        }
    }

    /// A pane no longer shows `id`. The detail is returned when this ends
    /// it: nothing else shows it.
    pub(in crate::workspace) fn release(&mut self, id: PrDetailId) -> Option<PrDetailState> {
        let held = self.panes.iter().position(|held| *held == id)?;
        self.panes.swap_remove(held);
        self.end_if_unshown(id)
    }

    fn end_if_unshown(&mut self, id: PrDetailId) -> Option<PrDetailState> {
        if self.page == Some(id) || self.panes.contains(&id) {
            return None;
        }
        let at = self.open.iter().position(|detail| detail.id() == id)?;
        Some(self.open.swap_remove(at))
    }

    #[cfg(test)]
    pub(in crate::workspace) fn len(&self) -> usize {
        self.open.len()
    }
}

#[cfg(test)]
mod tests;
