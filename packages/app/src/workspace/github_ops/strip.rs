//! What the Git Changes panel's pull-request strip says for the active lane.

use daruda_github::availability::GhAvailability;
use daruda_github::{GITHUB_HOST, RepoId};
use gpui::App;

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
    Linked(PrLink),
}

/// Decide the strip from what is known. Pure, so every combination is
/// testable without a window. Create PR is offered only once GitHub has
/// answered that this branch, not the default one, has no PR.
pub(crate) fn strip_for(
    enabled: bool,
    repo: Option<RepoId>,
    availability: GhAvailability,
    lookup: &PrLookup,
) -> PrStrip {
    let Some(repo) = repo.filter(|_| enabled) else {
        return PrStrip::Hidden;
    };
    let on_github = repo.host() == GITHUB_HOST;
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
            .and_then(|state| state.github_push())
            .map(|(repo, _)| repo);
        let availability = repo.as_ref().map_or(GhAvailability::Unknown, |repo| {
            PrPoller::availability(repo.host(), cx)
        });
        let unknown = PrLookup::Unknown;
        let lookup = state.map_or(&unknown, |state| state.pr.lookup());
        strip_for(enabled, repo, availability, lookup)
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
    use daruda_github::{CheckRollup, PrState, PrSummary};

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
                strip_for(enabled, repo, availability, &lookup),
                expected,
                "{label}"
            );
        }
    }
}
