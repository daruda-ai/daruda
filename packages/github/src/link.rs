//! Which pull request a lane's branch belongs to.

use crate::{PrSummary, RepoId};

/// The pull request a review branch was checked out for, as daruda records
/// it on the branch: `host/owner/name#number`. A fork's head cannot be
/// found by branch name in the base repository, so the branch names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewMark {
    pub repo: RepoId,
    pub number: u64,
}

impl ReviewMark {
    pub fn encode(&self) -> String {
        format!("{}#{}", self.repo.qualified(), self.number)
    }

    /// `None` for anything [`Self::encode`] could not have written.
    pub fn parse(raw: &str) -> Option<Self> {
        let (repo, number) = raw.rsplit_once('#')?;
        let number = number.parse().ok().filter(|n| *n > 0)?;
        let mut parts = repo.split('/');
        let (host, owner, name) = (parts.next()?, parts.next()?, parts.next()?);
        let complete = parts.next().is_none() && ![host, owner, name].contains(&"");
        complete.then(|| Self {
            repo: RepoId::new(host, owner, name),
            number,
        })
    }
}

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
    fn a_review_mark_reads_back_what_it_wrote_and_nothing_else() {
        let mark = ReviewMark {
            repo: RepoId::new("github.com", "O", "r"),
            number: 9,
        };
        assert_eq!(mark.encode(), "github.com/o/r#9");
        assert_eq!(ReviewMark::parse(&mark.encode()), Some(mark));
        // (label, raw)
        for (label, raw) in [
            ("no number", "github.com/o/r"),
            ("number zero", "github.com/o/r#0"),
            ("not a number", "github.com/o/r#x"),
            ("no host", "o/r#9"),
            ("an extra part", "github.com/o/r/x#9"),
            ("an empty part", "github.com//r#9"),
            ("empty", ""),
        ] {
            assert_eq!(ReviewMark::parse(raw), None, "{label}");
        }
    }

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
