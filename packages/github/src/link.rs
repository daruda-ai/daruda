//! Which pull request a lane's branch belongs to.

use crate::PrSummary;

/// The pull request `branch` belongs to, among `candidates` whose head
/// branch GitHub matched by name.
///
/// Only same-repository PRs count: a fork's `main` is not this repository's
/// `main`. A lane on the default branch has no PR of its own. An open or draft
/// PR wins over closed ones; otherwise the most recently updated wins.
pub fn pick<'a>(
    branch: &str,
    default_branch: &str,
    candidates: &'a [PrSummary],
) -> Option<&'a PrSummary> {
    if branch == default_branch {
        return None;
    }
    candidates
        .iter()
        .filter(|pr| !pr.cross_repo && pr.head_branch == branch)
        .max_by(|a, b| {
            (a.state.is_active(), &a.updated_at).cmp(&(b.state.is_active(), &b.updated_at))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CheckRollup, PrState};

    #[test]
    fn nothing_to_pick_from_is_none() {
        assert!(pick("feat", "main", &[]).is_none());
        let pr = PrSummary {
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
        };
        assert_eq!(pick("feat", "main", std::slice::from_ref(&pr)), Some(&pr));
    }
}
