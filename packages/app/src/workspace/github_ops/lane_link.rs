//! How a lane is tied to its pull request: decided once, from where its
//! branch pushes and what the user decided for its worktree. Everything
//! that asks which pull request a lane works on, why, or whether it may be
//! tied to another reads this, so the order (review mark, then the user's
//! choice, then the branch name) lives in one place.

use daruda_github::RepoId;
use daruda_github::link::ReviewMark;
use daruda_github::remote_parse::parse_remote;
use daruda_store::pr_links::WorktreeLinks;

use crate::github::PrSubject;
use crate::github::links::{Source, pr_key};
use crate::lane::git::PushTarget;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) enum LaneLink {
    /// A review branch whose mark daruda cannot read: nothing is asked for
    /// it, and it is not tied to another pull request either.
    UnreadableReview,
    /// A review branch: the pull request its mark names.
    Review { repo: RepoId, number: u64 },
    /// The pull request the user chose for the worktree's branch.
    Chosen { repo: RepoId, number: u64 },
    /// Whichever pull request the pushed branch heads in `repo`.
    Branch { repo: RepoId, branch: String },
}

impl LaneLink {
    /// `None` without a push target, or for a branch left to its name that
    /// pushes somewhere other than a GitHub-shaped remote.
    pub(in crate::workspace) fn resolve(
        push: Option<&PushTarget>,
        links: Option<&WorktreeLinks>,
    ) -> Option<Self> {
        let push = push?;
        if let Some(raw) = push.review.as_deref() {
            return Some(match ReviewMark::parse(raw) {
                Some(mark) => Self::Review {
                    repo: mark.repo,
                    number: mark.number,
                },
                None => Self::UnreadableReview,
            });
        }
        if let Some(chosen) = links.and_then(|links| links.chosen.as_ref()) {
            let (repo, number) = pr_key(chosen);
            return Some(Self::Chosen { repo, number });
        }
        Some(Self::Branch {
            repo: parse_remote(&push.url)?,
            branch: push.branch.clone(),
        })
    }

    /// What the poller is asked for the lane. A review's or a chosen pull
    /// request is asked for by number: its head need not be this branch's
    /// name, or even in this repository.
    pub(in crate::workspace) fn subject(&self) -> Option<(RepoId, PrSubject)> {
        match self {
            Self::UnreadableReview => None,
            Self::Review { repo, number } | Self::Chosen { repo, number } => {
                Some((repo.clone(), PrSubject::Number(*number)))
            }
            Self::Branch { repo, branch } => {
                Some((repo.clone(), PrSubject::Branch(branch.clone())))
            }
        }
    }

    /// The pull request known before GitHub answers: one named by number.
    pub(in crate::workspace) fn known_key(&self) -> Option<(RepoId, u64)> {
        match self {
            Self::Review { repo, number } | Self::Chosen { repo, number } => {
                Some((repo.clone(), *number))
            }
            Self::UnreadableReview | Self::Branch { .. } => None,
        }
    }

    /// Why the lane is tied, given whether its branch name found one.
    pub(in crate::workspace) fn source(&self, found: bool) -> Option<Source> {
        match self {
            Self::UnreadableReview | Self::Review { .. } => Some(Source::Review),
            Self::Chosen { .. } => Some(Source::Chosen),
            Self::Branch { .. } => found.then_some(Source::Branch),
        }
    }

    /// A review worktree exists for the pull request it reviews: tying it to
    /// another, or untying it, would change nothing.
    pub(in crate::workspace) fn is_review(&self) -> bool {
        matches!(self, Self::UnreadableReview | Self::Review { .. })
    }
}

#[cfg(test)]
mod tests {
    use daruda_store::pr_links::PrRef;

    use super::*;

    fn push(review: Option<&str>) -> PushTarget {
        PushTarget {
            remote: "origin".into(),
            url: "git@github.com:o/r.git".into(),
            branch: "feat".into(),
            published: true,
            has_upstream: true,
            review: review.map(str::to_owned),
        }
    }

    fn chose(number: u64) -> WorktreeLinks {
        WorktreeLinks {
            worktree: "/w".into(),
            branch: "feat".into(),
            chosen: Some(PrRef {
                host: "github.com".into(),
                owner: "o".into(),
                name: "other".into(),
                number,
            }),
            suppressed: Vec::new(),
            history: Vec::new(),
        }
    }

    #[test]
    fn a_review_mark_then_a_choice_then_the_branch_name_decide() {
        let repo = RepoId::new("github.com", "o", "r");
        let other = RepoId::new("github.com", "o", "other");
        let mut elsewhere = push(None);
        elsewhere.url = "/srv/git/r.git".into();
        // (label, push, links, expected)
        let cases = [
            ("no push target", None, None, None),
            (
                "a review mark wins over a choice",
                Some(push(Some("github.com/o/r#4"))),
                Some(chose(9)),
                Some(LaneLink::Review {
                    repo: repo.clone(),
                    number: 4,
                }),
            ),
            (
                "an unreadable mark is still a review",
                Some(push(Some("garbage"))),
                Some(chose(9)),
                Some(LaneLink::UnreadableReview),
            ),
            (
                "a choice wins over the branch name",
                Some(push(None)),
                Some(chose(9)),
                Some(LaneLink::Chosen {
                    repo: other.clone(),
                    number: 9,
                }),
            ),
            (
                "the branch name",
                Some(push(None)),
                None,
                Some(LaneLink::Branch {
                    repo: repo.clone(),
                    branch: "feat".into(),
                }),
            ),
            (
                "a remote that names no repository",
                Some(elsewhere),
                None,
                None,
            ),
        ];
        for (label, push, links, expected) in cases {
            assert_eq!(
                LaneLink::resolve(push.as_ref(), links.as_ref()),
                expected,
                "{label}"
            );
        }
    }

    #[test]
    fn each_tie_answers_what_is_asked_why_and_whether_it_moves() {
        let repo = RepoId::new("github.com", "o", "r");
        let review = LaneLink::Review {
            repo: repo.clone(),
            number: 4,
        };
        let chosen = LaneLink::Chosen {
            repo: repo.clone(),
            number: 9,
        };
        let branch = LaneLink::Branch {
            repo: repo.clone(),
            branch: "feat".into(),
        };
        assert_eq!(LaneLink::UnreadableReview.subject(), None, "nothing asked");
        assert_eq!(review.subject(), Some((repo.clone(), PrSubject::Number(4))));
        assert_eq!(chosen.subject(), Some((repo.clone(), PrSubject::Number(9))));
        assert_eq!(
            branch.subject(),
            Some((repo.clone(), PrSubject::Branch("feat".into())))
        );
        assert_eq!(chosen.known_key(), Some((repo.clone(), 9)));
        assert_eq!(branch.known_key(), None, "known once GitHub answers");
        assert_eq!(branch.source(false), None);
        assert_eq!(branch.source(true), Some(Source::Branch));
        assert_eq!(
            LaneLink::UnreadableReview.source(false),
            Some(Source::Review)
        );
        assert!(review.is_review() && LaneLink::UnreadableReview.is_review());
        assert!(!chosen.is_review() && !branch.is_review());
    }
}
