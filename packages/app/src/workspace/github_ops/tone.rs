//! How a pull request reads at a glance, shared by the lane badge and the
//! Git Changes strip.

use daruda_github::{CheckRollup, PrSummary};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrTone {
    Passing,
    Failing,
    Running,
    /// Nothing to act on: no checks, or a PR that is merged or closed.
    Quiet,
}

impl PrTone {
    pub(crate) fn of(pr: &PrSummary) -> Self {
        // A draft runs CI too, and its failures are worth fixing.
        if !pr.state.is_active() {
            return Self::Quiet;
        }
        match pr.checks {
            CheckRollup::Passing => Self::Passing,
            CheckRollup::Failing { .. } => Self::Failing,
            CheckRollup::Pending => Self::Running,
            CheckRollup::None => Self::Quiet,
        }
    }
}

/// A lane's pull request on its row: what the sidebar needs and no more,
/// so the per-render snapshot copies three small fields per lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PrBadge {
    pub number: u64,
    pub tone: PrTone,
    /// Open or draft; a merged or closed PR's number reads quieter.
    pub active: bool,
}

impl PrBadge {
    pub(crate) fn of(pr: &PrSummary) -> Self {
        Self {
            number: pr.number,
            tone: PrTone::of(pr),
            active: pr.state.is_active(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use daruda_github::PrState;

    fn pr(state: PrState, checks: CheckRollup) -> PrSummary {
        PrSummary {
            number: 1,
            url: String::new(),
            title: String::new(),
            state,
            head_branch: String::new(),
            cross_repo: false,
            checks,
            failed: Vec::new(),
            review: None,
            updated_at: String::new(),
        }
    }

    #[test]
    fn only_an_active_pr_shows_its_checks() {
        let cases = [
            (PrState::Open, CheckRollup::Passing, PrTone::Passing),
            (
                PrState::Open,
                CheckRollup::Failing { failed: Some(2) },
                PrTone::Failing,
            ),
            (PrState::Open, CheckRollup::Pending, PrTone::Running),
            (PrState::Open, CheckRollup::None, PrTone::Quiet),
            (
                PrState::Draft,
                CheckRollup::Failing { failed: Some(1) },
                PrTone::Failing,
            ),
            (PrState::Merged, CheckRollup::Passing, PrTone::Quiet),
            (
                PrState::Closed,
                CheckRollup::Failing { failed: Some(1) },
                PrTone::Quiet,
            ),
        ];
        for (state, checks, expected) in cases {
            assert_eq!(
                PrTone::of(&pr(state, checks)),
                expected,
                "{state:?} {checks:?}"
            );
        }
    }
}
