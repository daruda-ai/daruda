//! One lane's pull request: what GitHub said about its branch, one created
//! for it that GitHub does not list yet, and what the strip's buttons have
//! under way. Fields are private so every change goes
//! through a transition here; `github_ops` is the only writer.

use crate::github::{PrLink, PrLookup};

/// Work on a lane's pull request that its buttons must not repeat.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum PrWork {
    #[default]
    Idle,
    /// Logs or comments are being read for the agent.
    Sending,
}

/// A pull request created for the lane that the poller has not linked yet:
/// Create PR is not offered again meanwhile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PendingPr {
    /// Its page is known: the strip opens it.
    Named(CreatedPr),
    /// GitHub answered with no page daruda opens: the strip holds Create PR
    /// until the link arrives. Not tied to a branch; a move clears it, since
    /// the moved lane's answer is forgotten.
    Unnamed,
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
    pending: Option<PendingPr>,
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

    /// Record what is known now, reporting whether it changed. A link, or
    /// the default branch (which has none), is done with the new pull
    /// request. Forgetting the answer (the branch or the means to ask is
    /// gone) ends an unnamed one's hold; a named one stays for its branch.
    pub(super) fn replace(&mut self, lookup: PrLookup) -> bool {
        match (&lookup, &self.pending) {
            (PrLookup::Linked(_) | PrLookup::DefaultBranch, _)
            | (PrLookup::Unknown, Some(PendingPr::Unnamed)) => self.pending = None,
            _ => {}
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

    /// Start reading logs or comments for the agent; offered only for a
    /// linked pull request.
    pub(super) fn begin_send(&mut self) {
        self.work = PrWork::Sending;
    }

    /// The send finished, whatever its outcome.
    pub(super) fn end_send(&mut self) {
        if self.work == PrWork::Sending {
            self.work = PrWork::Idle;
        }
    }

    /// A pull request was created, or found, for the lane: kept until the
    /// poller links it. The latest creation replaces an earlier one.
    pub(super) fn created(&mut self, pending: PendingPr) {
        self.pending = Some(pending);
    }

    /// [`Self::created`] for a window test.
    #[cfg(test)]
    pub(in crate::workspace) fn created_for_test(&mut self, created: CreatedPr) {
        self.created(PendingPr::Named(created));
    }

    pub(in crate::workspace) fn pending(&self) -> Option<&PendingPr> {
        self.pending.as_ref()
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
    fn a_pending_pr_is_held_until_linked() {
        let created = CreatedPr::of("https://github.com/o/r/pull/12".into(), "feat").unwrap();
        // (label, held, answers, still held)
        let cases = [
            (
                "named: not linked yet",
                PendingPr::Named(created.clone()),
                vec![PrLookup::NoPr, PrLookup::Unknown],
                true,
            ),
            (
                "unnamed: an answer without it keeps the hold",
                PendingPr::Unnamed,
                vec![PrLookup::NoPr],
                true,
            ),
            (
                "unnamed: a forgotten answer ends it",
                PendingPr::Unnamed,
                vec![PrLookup::Unknown],
                false,
            ),
            (
                "the default branch ends it",
                PendingPr::Named(created.clone()),
                vec![PrLookup::DefaultBranch],
                false,
            ),
        ];
        for (label, held, answers, kept) in cases {
            let mut pr = LanePr::default();
            pr.created(held.clone());
            for answer in answers {
                pr.replace(answer);
            }
            assert_eq!(pr.pending() == Some(&held), kept, "{label}");
        }
    }

    #[test]
    fn a_send_runs_until_it_ends() {
        let mut pr = LanePr::default();
        pr.begin_send();
        assert!(pr.is_sending());
        pr.end_send();
        assert_eq!(pr.work(), PrWork::Idle);
    }
}
