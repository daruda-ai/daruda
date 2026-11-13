//! One lane's pull request: what GitHub said about its branch and what the
//! strip's buttons have under way. Fields are private so every change goes
//! through a transition here; `github_ops` is the only writer.

use crate::github::{PrLink, PrLookup};

/// Work on a lane's pull request that its buttons must not repeat.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum PrWork {
    #[default]
    Idle,
    /// Logs or comments are being read for the agent.
    Sending,
    /// A PR was created and the poller has not linked it yet.
    AwaitingNewPr,
}

/// Never persisted: the next poll answers it again.
#[derive(Debug, Default)]
pub(in crate::workspace) struct LanePr {
    lookup: PrLookup,
    work: PrWork,
}

impl LanePr {
    pub(in crate::workspace) fn lookup(&self) -> &PrLookup {
        &self.lookup
    }

    pub(in crate::workspace) fn link(&self) -> Option<&PrLink> {
        self.lookup.link()
    }

    pub(in crate::workspace) fn work(&self) -> PrWork {
        self.work
    }

    /// Record what is known now, reporting whether it changed. A link
    /// arriving ends the wait after Create PR; so does forgetting the answer,
    /// since the branch or the means to ask about it is gone.
    pub(super) fn replace(&mut self, lookup: PrLookup) -> bool {
        if matches!(lookup, PrLookup::Linked(_) | PrLookup::Unknown)
            && self.work == PrWork::AwaitingNewPr
        {
            self.work = PrWork::Idle;
        }
        if self.lookup == lookup {
            return false;
        }
        self.lookup = lookup;
        true
    }

    pub(in crate::workspace) fn is_sending(&self) -> bool {
        self.work == PrWork::Sending
    }

    /// Start reading logs or comments for the agent. Sending needs a linked
    /// PR, and a link has already ended any wait after Create PR.
    pub(super) fn begin_send(&mut self) {
        self.work = PrWork::Sending;
    }

    /// The send finished, whatever its outcome.
    pub(super) fn end_send(&mut self) {
        if self.work == PrWork::Sending {
            self.work = PrWork::Idle;
        }
    }

    /// A PR was created; hold Create PR until the poller links it.
    pub(super) fn await_new_pr(&mut self) {
        self.work = PrWork::AwaitingNewPr;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_or_a_forgotten_answer_ends_the_wait_after_create() {
        let mut pr = LanePr::default();
        pr.await_new_pr();
        pr.replace(PrLookup::NoPr);
        assert_eq!(
            pr.work(),
            PrWork::AwaitingNewPr,
            "an answer without it yet keeps waiting"
        );
        pr.replace(PrLookup::Unknown);
        assert_eq!(pr.work(), PrWork::Idle);
    }

    #[test]
    fn a_send_runs_until_it_ends() {
        let mut pr = LanePr::default();
        pr.begin_send();
        assert!(pr.is_sending());
        pr.end_send();
        assert_eq!(pr.work(), PrWork::Idle);
    }

    #[test]
    fn ending_a_send_leaves_a_create_wait_alone() {
        let mut pr = LanePr::default();
        pr.await_new_pr();
        pr.end_send();
        assert_eq!(pr.work(), PrWork::AwaitingNewPr);
    }
}
