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

/// A pull request daruda created for the lane, kept until the poller
/// links it: whatever the lookup says meanwhile, Create PR is not offered
/// again for a branch that has one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CreatedPr {
    /// A web page: nothing else is ever opened.
    pub url: String,
    /// From the URL; `None` when it does not end in one.
    pub number: Option<u64>,
    /// The branch it is for, as pushed: another branch is not offered it.
    pub head: String,
}

impl CreatedPr {
    /// `None` for a URL that is not a web page.
    pub(crate) fn of(url: String, head: &str) -> Option<Self> {
        if !daruda_github::model::is_web_url(&url) {
            return None;
        }
        let number = url
            .trim_end_matches('/')
            .rsplit_once("/pull/")
            .and_then(|(_, number)| number.parse().ok());
        Some(Self {
            url,
            number,
            head: head.to_owned(),
        })
    }
}

/// Never persisted: the next poll answers it again.
#[derive(Debug, Default)]
pub(in crate::workspace) struct LanePr {
    lookup: PrLookup,
    work: PrWork,
    created: Option<CreatedPr>,
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
        // Linked, or on the default branch (which has none): done with.
        if matches!(lookup, PrLookup::Linked(_) | PrLookup::DefaultBranch) {
            self.created = None;
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

    /// Remember the pull request created for the lane until it is linked.
    pub(super) fn created(&mut self, created: CreatedPr) {
        self.created = Some(created);
    }

    /// [`Self::created`] for a window test.
    #[cfg(test)]
    pub(in crate::workspace) fn created_for_test(&mut self, created: CreatedPr) {
        self.created(created);
    }

    pub(in crate::workspace) fn created_pr(&self) -> Option<&CreatedPr> {
        self.created.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_created_pr_is_a_web_page_numbered_from_its_url() {
        // (label, url, number, or None if refused)
        let cases = [
            (
                "a pull request",
                "https://github.com/o/r/pull/12",
                Some(Some(12)),
            ),
            (
                "a trailing slash",
                "https://github.com/o/r/pull/12/",
                Some(Some(12)),
            ),
            (
                "a sub-page",
                "https://github.com/o/r/pull/12/files",
                Some(None),
            ),
            ("no number", "https://x/o/r", Some(None)),
            ("not a web page", "javascript:alert(1)", None),
            ("a file", "file:///etc/passwd", None),
        ];
        for (label, url, expected) in cases {
            assert_eq!(
                CreatedPr::of(url.into(), "feat").map(|created| created.number),
                expected,
                "{label}"
            );
        }
    }

    #[test]
    fn a_created_pr_is_held_until_linked() {
        let created = CreatedPr::of("https://github.com/o/r/pull/12".into(), "feat").unwrap();
        let mut pr = LanePr::default();
        pr.created(created.clone());
        pr.replace(PrLookup::NoPr);
        pr.replace(PrLookup::Unknown);
        assert_eq!(pr.created_pr(), Some(&created), "not linked yet");
    }

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
