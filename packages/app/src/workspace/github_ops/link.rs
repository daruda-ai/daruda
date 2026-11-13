//! Which pull request each lane belongs to. `LaneScoped::pr` changes only
//! through [`LanePr::replace`](super::lane_pr::LanePr::replace), from the
//! moments that can change it: a poll answer, a moved push target, `gh`
//! becoming unusable, and the feature being switched off.

use daruda_github::query::RepoPrs;
use daruda_github::remote_parse::parse_remote;
use daruda_github::{PrSummary, RepoId, link::pick};
use daruda_store::project::LaneRef;
use gpui::Context;

use crate::github::{PrDemand, PrLink, PrLookup, PrUpdate, is_unusable};
use crate::lane::git::PushTarget;
use crate::workspace::Workspace;
use crate::workspace::lane_scoped::LaneScoped;

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

impl LaneScoped {
    /// Where the lane's branch pushes, with the GitHub repository its remote
    /// names; `None` without a push target or for a remote that is not one.
    pub(in crate::workspace) fn github_push(&self) -> Option<(RepoId, &PushTarget)> {
        let push = self.git.push_target.as_ref()?;
        Some((parse_remote(&push.url)?, push))
    }
}

impl Workspace {
    /// Carry out what the poller says about the lanes' pull requests.
    pub(crate) fn apply_pr_update(&mut self, update: PrUpdate<'_>, cx: &mut Context<Self>) {
        match update {
            PrUpdate::Answer { repo, prs } => self.apply_repo_prs(repo, prs, cx),
            PrUpdate::Host { host, availability } => {
                if is_unusable(availability) {
                    self.forget_pr_links_on(host, cx);
                }
                // The strip shows the host's state even when no link changed.
                cx.notify();
            }
            PrUpdate::Enabled => self.refresh_push_targets(cx),
            PrUpdate::Disabled => self.clear_pr_links(cx),
        }
    }

    /// Every lane whose push target names a GitHub-shaped repository.
    pub(crate) fn pr_demand(&self) -> Vec<PrDemand> {
        self.lane_scoped
            .iter()
            .filter_map(|(target, state)| {
                let (repo, push) = state.github_push()?;
                Some(PrDemand {
                    repo,
                    branch: push.branch.clone(),
                    focused: *target == self.active,
                })
            })
            .collect()
    }

    /// Apply a poll answer for `repo` to every lane that pushes there. A lane
    /// whose branch the answer does not cover (it moved while the request
    /// ran) keeps what it had until the next answer that does.
    fn apply_repo_prs(&mut self, repo: &RepoId, prs: &RepoPrs, cx: &mut Context<Self>) {
        let mut changed = false;
        for state in self.lane_scoped.values_mut() {
            let Some((lane_repo, push)) = state.github_push() else {
                continue;
            };
            if lane_repo != *repo {
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
            changed |= state.pr.replace(lookup);
        }
        if changed {
            cx.notify();
        }
    }

    /// Re-read every git lane's push target: the feature was switched on,
    /// and push targets are not kept current while it is off.
    fn refresh_push_targets(&mut self, cx: &mut Context<Self>) {
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
        if self.lane_scoped_mut(lane).pr.replace(lookup) {
            cx.notify();
        }
    }

    /// Forget every lane's PR: the feature was switched off.
    fn clear_pr_links(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        for state in self.lane_scoped.values_mut() {
            changed |= state.pr.replace(PrLookup::Unknown);
        }
        if changed {
            cx.notify();
        }
    }

    /// Forget the PRs of lanes pushing to `host`: `gh` there is gone or
    /// logged out, so what was last read can no longer be kept current.
    fn forget_pr_links_on(&mut self, host: &str, cx: &mut Context<Self>) {
        let mut changed = false;
        for state in self.lane_scoped.values_mut() {
            let on_host = state
                .github_push()
                .is_some_and(|(repo, _)| repo.host() == host);
            if on_host {
                changed |= state.pr.replace(PrLookup::Unknown);
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
        state.git.push_target = found;
        if moved {
            state.pr.replace(PrLookup::Unknown);
            if let Some((repo, _)) = state.github_push() {
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
        RepoId::new("github.com", "o", "r")
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
}
