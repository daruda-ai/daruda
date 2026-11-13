//! Which pull request each lane belongs to. `LaneScoped::pr` changes only
//! through [`replace_pr`], from the moments that can change it: a poll
//! answer, a moved push target, `gh` becoming unusable, and the feature
//! being switched off.

use daruda_github::query::RepoPrs;
use daruda_github::remote_parse::parse_remote;
use daruda_github::{PrSummary, RepoId, link::pick};
use daruda_store::project::LaneRef;
use gpui::Context;

use crate::github::{PrLink, PrLookup};
use crate::lane::git::PushTarget;
use crate::workspace::Workspace;
use crate::workspace::lane_scoped::{LaneScoped, PrWork};

/// One lane's question for the poller: which PR is `branch` in `repo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrDemand {
    pub repo: RepoId,
    pub branch: String,
    /// The lane is the one this window shows, so it is polled more often.
    pub focused: bool,
}

/// What an answer says about `branch`. A repository without commits has no
/// default branch; its branches can still have, or lack, a PR.
fn lookup_for(
    repo: &RepoId,
    branch: &str,
    default_branch: Option<&str>,
    candidates: &[PrSummary],
) -> PrLookup {
    if default_branch == Some(branch) {
        return PrLookup::DefaultBranch;
    }
    match pick(branch, default_branch.unwrap_or_default(), candidates) {
        Some(pr) => PrLookup::Linked(Box::new(PrLink {
            repo: repo.clone(),
            pr: pr.clone(),
        })),
        None => PrLookup::NoPr,
    }
}

/// Set `state.pr`, reporting whether it changed. A link arriving ends the
/// wait after Create PR; so does forgetting the answer, since the branch or
/// the means to ask about it is gone.
fn replace_pr(state: &mut LaneScoped, lookup: PrLookup) -> bool {
    if matches!(lookup, PrLookup::Linked(_) | PrLookup::Unknown)
        && state.pr_work == PrWork::AwaitingNewPr
    {
        state.pr_work = PrWork::Idle;
    }
    if state.pr == lookup {
        return false;
    }
    state.pr = lookup;
    true
}

impl Workspace {
    /// Every lane whose push target names a GitHub-shaped repository.
    pub(crate) fn pr_demand(&self) -> Vec<PrDemand> {
        self.lane_scoped
            .iter()
            .filter_map(|(target, state)| {
                let push = state.git.push_target.as_ref()?;
                Some(PrDemand {
                    repo: parse_remote(&push.url)?,
                    branch: push.branch.clone(),
                    focused: *target == self.active,
                })
            })
            .collect()
    }

    /// Apply a poll answer for `repo` to every lane that pushes there. A lane
    /// whose branch the answer does not cover (it moved while the request
    /// ran) keeps what it had until the next answer that does.
    pub(crate) fn apply_repo_prs(&mut self, repo: &RepoId, prs: &RepoPrs, cx: &mut Context<Self>) {
        let mut changed = false;
        for state in self.lane_scoped.values_mut() {
            let Some(push) = state.git.push_target.as_ref() else {
                continue;
            };
            if parse_remote(&push.url).as_ref() != Some(repo) {
                continue;
            }
            let Some(candidates) = prs.by_branch.get(&push.branch) else {
                continue;
            };
            let lookup = lookup_for(
                repo,
                &push.branch,
                prs.default_branch.as_deref(),
                candidates,
            );
            changed |= replace_pr(state, lookup);
        }
        if changed {
            cx.notify();
        }
    }

    /// Re-read every git lane's push target: the feature was switched on,
    /// and push targets are not kept current while it is off.
    pub(crate) fn refresh_push_targets(&mut self, cx: &mut Context<Self>) {
        let lanes: Vec<LaneRef> = self.lane_scoped.keys().copied().collect();
        for lane in lanes {
            self.refresh_push_target(lane, cx);
        }
    }

    /// Set what a capture shows for a lane, through the same write as a poll.
    #[cfg(feature = "screenshot")]
    pub(in crate::workspace) fn seed_pr_lookup_for_shot(
        &mut self,
        lane: LaneRef,
        lookup: PrLookup,
        cx: &mut Context<Self>,
    ) {
        if replace_pr(self.lane_scoped_mut(lane), lookup) {
            cx.notify();
        }
    }

    /// Forget every lane's PR: the feature was switched off.
    pub(crate) fn clear_pr_links(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        for state in self.lane_scoped.values_mut() {
            changed |= replace_pr(state, PrLookup::Unknown);
        }
        if changed {
            cx.notify();
        }
    }

    /// Forget the PRs of lanes pushing to `host`: `gh` there is gone or
    /// logged out, so what was last read can no longer be kept current.
    pub(crate) fn forget_pr_links_on(&mut self, host: &str, cx: &mut Context<Self>) {
        let mut changed = false;
        for state in self.lane_scoped.values_mut() {
            let on_host = state
                .git
                .push_target
                .as_ref()
                .and_then(|push| parse_remote(&push.url))
                .is_some_and(|repo| repo.host == host);
            if on_host {
                changed |= replace_pr(state, PrLookup::Unknown);
            }
        }
        if changed {
            cx.notify();
        }
    }

    /// Record where `target` pushes. A different repository or branch drops
    /// the old answer and asks the poller about the new one at once.
    /// Returns whether anything changed.
    pub(in crate::workspace) fn set_push_target(
        &mut self,
        target: LaneRef,
        found: Option<PushTarget>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(state) = self.lane_scoped.get_mut(&target) else {
            return false;
        };
        if state.git.push_target == found {
            return false;
        }
        let moved = match (&state.git.push_target, &found) {
            (Some(old), Some(new)) => old.url != new.url || old.branch != new.branch,
            _ => true,
        };
        state.git.push_target = found.clone();
        if moved {
            replace_pr(state, PrLookup::Unknown);
            if let Some(repo) = found.as_ref().and_then(|push| parse_remote(&push.url)) {
                crate::github::PrPoller::nudge(repo, cx);
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use daruda_github::{CheckRollup, PrState};

    use super::*;

    fn repo() -> RepoId {
        RepoId {
            host: "github.com".into(),
            owner: "o".into(),
            name: "r".into(),
        }
    }

    fn pr(head: &str) -> PrSummary {
        PrSummary {
            number: 3,
            url: String::new(),
            title: String::new(),
            state: PrState::Open,
            head_branch: head.into(),
            cross_repo: false,
            checks: CheckRollup::None,
            failed: Vec::new(),
            review: None,
            updated_at: String::new(),
        }
    }

    #[test]
    fn an_answer_reads_as_default_branch_no_pr_or_a_link() {
        /// (label, branch, default branch, candidates, expected)
        type Row = (
            &'static str,
            &'static str,
            Option<&'static str>,
            Vec<PrSummary>,
            PrLookup,
        );
        let cases: Vec<Row> = vec![
            (
                "default branch",
                "main",
                Some("main"),
                vec![pr("main")],
                PrLookup::DefaultBranch,
            ),
            ("no PR", "feat", Some("main"), vec![], PrLookup::NoPr),
            (
                "linked",
                "feat",
                Some("main"),
                vec![pr("feat")],
                PrLookup::Linked(Box::new(PrLink {
                    repo: repo(),
                    pr: pr("feat"),
                })),
            ),
            (
                "repository without commits",
                "feat",
                None,
                vec![],
                PrLookup::NoPr,
            ),
        ];
        for (label, branch, default, candidates, expected) in cases {
            assert_eq!(
                lookup_for(&repo(), branch, default, &candidates),
                expected,
                "{label}"
            );
        }
    }

    #[test]
    fn a_link_or_a_forgotten_answer_ends_the_wait_after_create() {
        let mut state = LaneScoped {
            pr_work: PrWork::AwaitingNewPr,
            ..LaneScoped::default()
        };
        replace_pr(&mut state, PrLookup::NoPr);
        assert_eq!(
            state.pr_work,
            PrWork::AwaitingNewPr,
            "an answer without it yet keeps waiting"
        );
        replace_pr(&mut state, PrLookup::Unknown);
        assert_eq!(state.pr_work, PrWork::Idle);
    }
}
