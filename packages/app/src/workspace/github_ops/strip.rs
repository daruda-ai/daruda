//! What the Git Changes panel's pull-request strip says for the active lane.

use daruda_github::RepoId;
use daruda_github::availability::GhAvailability;
use daruda_github::remote_parse::parse_remote;
use gpui::App;

use crate::github::{PrLink, PrPoller};
use crate::workspace::Workspace;

/// The one host whose `gh` trouble is worth telling the user about unasked:
/// a remote on another host may not be GitHub at all.
const GITHUB_HOST: &str = "github.com";

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
/// testable without a window.
pub(crate) fn strip_for(
    enabled: bool,
    repo: Option<RepoId>,
    availability: GhAvailability,
    link: Option<PrLink>,
) -> PrStrip {
    let Some(repo) = repo.filter(|_| enabled) else {
        return PrStrip::Hidden;
    };
    let on_github = repo.host == GITHUB_HOST;
    match availability {
        GhAvailability::Unknown => PrStrip::Hidden,
        GhAvailability::Missing if on_github => PrStrip::Missing { host: repo.host },
        GhAvailability::LoggedOut if on_github => PrStrip::LoggedOut { host: repo.host },
        GhAvailability::Missing | GhAvailability::LoggedOut => PrStrip::Hidden,
        GhAvailability::Ready | GhAvailability::Backoff { .. } => match link {
            Some(link) => PrStrip::Linked(link),
            None => PrStrip::NoPr,
        },
    }
}

impl Workspace {
    /// The strip for the active lane.
    pub(in crate::workspace) fn pr_strip(&self, cx: &App) -> PrStrip {
        let enabled = crate::settings_store::SettingsStore::global(cx)
            .user()
            .github
            .enabled;
        let state = self.lane_scoped.get(&self.active);
        let repo = state
            .and_then(|state| state.git.push_target.as_ref())
            .and_then(|push| parse_remote(&push.url));
        let availability = repo.as_ref().map_or(GhAvailability::Unknown, |repo| {
            PrPoller::availability(&repo.host, cx)
        });
        let link = state.and_then(|state| state.pr.clone());
        strip_for(enabled, repo, availability, link)
    }

    /// Open the active lane's pull request on GitHub.
    pub(in crate::workspace) fn open_pr_in_browser(&mut self, cx: &mut App) {
        if let Some(link) = self
            .lane_scoped
            .get(&self.active)
            .and_then(|state| state.pr.as_ref())
        {
            cx.open_url(&link.pr.url);
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
        Some(RepoId {
            host: host.into(),
            owner: "o".into(),
            name: "r".into(),
        })
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

    /// `(label, enabled, repo, availability, link, expected)`.
    type Case = (
        &'static str,
        bool,
        Option<RepoId>,
        GhAvailability,
        Option<PrLink>,
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
            ("off", false, repo(gh), Ready, Some(link()), PrStrip::Hidden),
            ("no push target", true, None, Ready, None, PrStrip::Hidden),
            (
                "not asked yet",
                true,
                repo(gh),
                Unknown,
                None,
                PrStrip::Hidden,
            ),
            (
                "gh missing",
                true,
                repo(gh),
                Missing,
                None,
                PrStrip::Missing { host: gh.into() },
            ),
            (
                "logged out",
                true,
                repo(gh),
                LoggedOut,
                None,
                PrStrip::LoggedOut { host: gh.into() },
            ),
            (
                "another host without gh login",
                true,
                repo("gitlab.com"),
                LoggedOut,
                None,
                PrStrip::Hidden,
            ),
            (
                "another host gh knows",
                true,
                repo("ghe.corp"),
                Ready,
                None,
                PrStrip::NoPr,
            ),
            ("no PR yet", true, repo(gh), Ready, None, PrStrip::NoPr),
            (
                "linked",
                true,
                repo(gh),
                Ready,
                Some(link()),
                PrStrip::Linked(link()),
            ),
            (
                "a pause keeps the last link",
                true,
                repo(gh),
                backoff,
                Some(link()),
                PrStrip::Linked(link()),
            ),
        ];
        for (label, enabled, repo, availability, link, expected) in cases {
            assert_eq!(
                strip_for(enabled, repo, availability, link),
                expected,
                "{label}"
            );
        }
    }
}
