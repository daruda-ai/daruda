//! What the Git Changes panel's pull-request strip says for the active lane.

use daruda_github::RepoId;
use daruda_github::availability::GhAvailability;
use gpui::App;

use super::lane_pr::PendingPr;
use crate::github::{PrLink, PrLookup, PrPoller};
use crate::workspace::Workspace;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PrStrip {
    /// Nothing to show: off, not a GitHub remote, or not known yet.
    Hidden,
    /// `gh` is not installed.
    Missing {
        host: String,
    },
    /// `gh` has no login for the host.
    LoggedOut {
        host: String,
    },
    /// GitHub answers, and this branch has no pull request.
    NoPr,
    /// One was created that GitHub named no page for: Create PR is held
    /// until the poller links it.
    Awaiting,
    /// daruda created one that GitHub's lists do not show yet: Open it,
    /// never Create another.
    Created(super::lane_pr::CreatedPr),
    /// The user untied the pull request its branch name finds: tie it
    /// again rather than offer creating one GitHub already has.
    Untied {
        number: u64,
    },
    Linked(PrLink),
}

/// What is known about the active lane's pull request.
#[derive(Debug, Clone, Copy)]
pub(crate) struct StripFacts<'a> {
    pub enabled: bool,
    pub availability: GhAvailability,
    pub lookup: &'a PrLookup,
    /// One created for the lane and not linked yet.
    pub pending: Option<&'a PendingPr>,
    /// The branch the lane pushes now.
    pub head: Option<&'a str>,
}

/// Decide the strip from what is known. Pure, so every combination is
/// testable without a window. Create PR is offered only once GitHub has
/// answered that this branch, not the default one, has no PR, and never
/// again for a branch daruda just created one for.
pub(crate) fn strip_for(repo: Option<RepoId>, facts: StripFacts<'_>) -> PrStrip {
    match (
        answered_strip(facts.enabled, repo, facts.availability, facts.lookup),
        facts.pending,
    ) {
        // The untied one itself, found again, is offered to tie again.
        (PrStrip::Untied { number }, Some(PendingPr::Named(created)))
            if created.number == Some(number) =>
        {
            PrStrip::Untied { number }
        }
        (PrStrip::NoPr | PrStrip::Untied { .. }, Some(PendingPr::Named(created)))
            if facts.head == Some(created.head.as_str()) =>
        {
            PrStrip::Created(created.clone())
        }
        (PrStrip::NoPr, Some(PendingPr::Unnamed { head })) if facts.head == Some(head.as_str()) => {
            PrStrip::Awaiting
        }
        (strip, _) => strip,
    }
}

/// The strip from GitHub's answer alone.
fn answered_strip(
    enabled: bool,
    repo: Option<RepoId>,
    availability: GhAvailability,
    lookup: &PrLookup,
) -> PrStrip {
    let Some(repo) = repo.filter(|_| enabled) else {
        return PrStrip::Hidden;
    };
    // Off github.com the strip stays quiet: the host may not be GitHub at
    // all, and nobody asked. The Pull requests page says so instead.
    let on_github = repo.on_github();
    match availability {
        GhAvailability::Unknown => PrStrip::Hidden,
        GhAvailability::Missing if on_github => PrStrip::Missing {
            host: repo.host().to_owned(),
        },
        GhAvailability::LoggedOut if on_github => PrStrip::LoggedOut {
            host: repo.host().to_owned(),
        },
        GhAvailability::Missing | GhAvailability::LoggedOut => PrStrip::Hidden,
        GhAvailability::Ready | GhAvailability::Backoff { .. } => match lookup {
            PrLookup::Linked(link) => PrStrip::Linked(PrLink::clone(link)),
            PrLookup::NoPr => PrStrip::NoPr,
            PrLookup::Untied(number) => PrStrip::Untied { number: *number },
            PrLookup::Unknown | PrLookup::DefaultBranch => PrStrip::Hidden,
        },
    }
}

impl Workspace {
    /// The strip for the active lane.
    pub(in crate::workspace) fn pr_strip(&self, cx: &App) -> PrStrip {
        let enabled = crate::github::enabled(cx);
        let state = self.lane_scoped.get(&self.active);
        let repo = state
            .and_then(|state| state.pr_subject())
            .map(|(repo, _)| repo);
        let availability = repo.as_ref().map_or(GhAvailability::Unknown, |repo| {
            PrPoller::availability(repo.host(), cx)
        });
        let unknown = PrLookup::Unknown;
        let head = state.and_then(|state| state.github_push().map(|(_, push)| push.branch.clone()));
        strip_for(
            repo,
            StripFacts {
                enabled,
                availability,
                lookup: state.map_or(&unknown, |state| state.pr.lookup()),
                pending: state.and_then(|state| state.pr.pending()),
                head: head.as_deref(),
            },
        )
    }

    /// Open the active lane's pull request on GitHub.
    pub(in crate::workspace) fn open_pr_in_browser(&mut self, cx: &mut App) {
        if let Some(url) = self
            .lane_scoped
            .get(&self.active)
            .and_then(|state| state.pr.link())
            .and_then(|link| link.pr.web_url())
        {
            cx.open_url(url);
        }
    }

    /// Ask `gh` about `host` again: the user installed it or logged in.
    pub(in crate::workspace) fn recheck_gh(&mut self, host: &str, cx: &mut App) {
        PrPoller::recheck(host, cx);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use daruda_github::availability::BackoffReason;
    use daruda_github::{CheckRollup, GITHUB_HOST, PrState, PrSummary};

    use super::*;

    fn repo(host: &str) -> Option<RepoId> {
        Some(RepoId::new(host, "o", "r"))
    }

    fn link() -> PrLink {
        PrLink {
            repo: repo(GITHUB_HOST).unwrap(),
            pr: PrSummary {
                number: 1,
                url: String::new(),
                title: String::new(),
                state: PrState::Open,
                head_branch: "feat".into(),
                cross_repo: false,
                checks: CheckRollup::None,
                failed: Vec::new(),
                review: None,
                updated_at: String::new(),
            },
        }
    }

    /// `(label, enabled, repo, availability, lookup, expected)`.
    type Case = (
        &'static str,
        bool,
        Option<RepoId>,
        GhAvailability,
        PrLookup,
        PrStrip,
    );

    #[test]
    fn the_strip_shows_only_what_is_worth_saying() {
        use GhAvailability::*;
        let backoff = Backoff {
            until: Instant::now(),
            reason: BackoffReason::Transient,
            resume: daruda_github::availability::Resume::Estimated,
        };
        let gh = GITHUB_HOST;
        let cases: Vec<Case> = vec![
            (
                "off",
                false,
                repo(gh),
                Ready,
                PrLookup::Linked(Box::new(link())),
                PrStrip::Hidden,
            ),
            (
                "no push target",
                true,
                None,
                Ready,
                PrLookup::NoPr,
                PrStrip::Hidden,
            ),
            (
                "not asked yet",
                true,
                repo(gh),
                Unknown,
                PrLookup::Unknown,
                PrStrip::Hidden,
            ),
            (
                "gh missing",
                true,
                repo(gh),
                Missing,
                PrLookup::Unknown,
                PrStrip::Missing { host: gh.into() },
            ),
            (
                "logged out",
                true,
                repo(gh),
                LoggedOut,
                PrLookup::Unknown,
                PrStrip::LoggedOut { host: gh.into() },
            ),
            (
                "another host without gh login",
                true,
                repo("gitlab.com"),
                LoggedOut,
                PrLookup::Unknown,
                PrStrip::Hidden,
            ),
            (
                "another host gh knows",
                true,
                repo("ghe.corp"),
                Ready,
                PrLookup::NoPr,
                PrStrip::NoPr,
            ),
            (
                "no PR yet",
                true,
                repo(gh),
                Ready,
                PrLookup::NoPr,
                PrStrip::NoPr,
            ),
            (
                "untied and still open: tie it again, no Create PR",
                true,
                repo(gh),
                Ready,
                PrLookup::Untied(7),
                PrStrip::Untied { number: 7 },
            ),
            (
                "not answered yet: no Create PR",
                true,
                repo(gh),
                Ready,
                PrLookup::Unknown,
                PrStrip::Hidden,
            ),
            (
                "the default branch has no PR to create",
                true,
                repo(gh),
                Ready,
                PrLookup::DefaultBranch,
                PrStrip::Hidden,
            ),
            (
                "linked",
                true,
                repo(gh),
                Ready,
                PrLookup::Linked(Box::new(link())),
                PrStrip::Linked(link()),
            ),
            (
                "a pause keeps the last link",
                true,
                repo(gh),
                backoff,
                PrLookup::Linked(Box::new(link())),
                PrStrip::Linked(link()),
            ),
        ];
        for (label, enabled, repo, availability, lookup, expected) in cases {
            assert_eq!(
                answered_strip(enabled, repo, availability, &lookup),
                expected,
                "{label}"
            );
        }
    }

    /// A pull request created for the lane, not linked yet: opened rather
    /// than created again, and only on the branch it was made for.
    #[test]
    fn a_created_pull_request_holds_create_until_linked() {
        let created = crate::workspace::github_ops::lane_pr::CreatedPr::of(
            "https://github.com/o/r/pull/9".into(),
            "feat",
        )
        .unwrap();
        let named = PendingPr::Named(created.clone());
        let unnamed = PendingPr::Unnamed {
            head: "feat".into(),
        };
        let ready = GhAvailability::Ready;
        // (label, lookup, pending, head, expected)
        let cases = [
            (
                "named, unlisted: open it",
                PrLookup::NoPr,
                Some(&named),
                Some("feat"),
                PrStrip::Created(created.clone()),
            ),
            (
                "named, untied one listed: open the new one",
                PrLookup::Untied(3),
                Some(&named),
                Some("feat"),
                PrStrip::Created(created.clone()),
            ),
            (
                "named, another branch now: create there",
                PrLookup::NoPr,
                Some(&named),
                Some("other"),
                PrStrip::NoPr,
            ),
            (
                "unnamed: hold Create PR",
                PrLookup::NoPr,
                Some(&unnamed),
                Some("feat"),
                PrStrip::Awaiting,
            ),
            (
                "unnamed, another branch now: create there",
                PrLookup::NoPr,
                Some(&unnamed),
                Some("other"),
                PrStrip::NoPr,
            ),
            (
                "named is the untied one: tie it again",
                PrLookup::Untied(9),
                Some(&named),
                Some("feat"),
                PrStrip::Untied { number: 9 },
            ),
            (
                "linked: the link wins",
                PrLookup::Linked(Box::new(link())),
                Some(&named),
                Some("feat"),
                PrStrip::Linked(link()),
            ),
            (
                "nothing created",
                PrLookup::NoPr,
                None,
                Some("feat"),
                PrStrip::NoPr,
            ),
        ];
        for (label, lookup, pending, head, expected) in cases {
            let facts = StripFacts {
                enabled: true,
                availability: ready,
                lookup: &lookup,
                pending,
                head,
            };
            assert_eq!(strip_for(repo(GITHUB_HOST), facts), expected, "{label}");
        }
    }
}
