//! Which pull request each lane belongs to. `LaneScoped::pr` changes only
//! through [`LanePr::replace`](super::lane_pr::LanePr::replace), from the
//! moments that can change it: a poll answer, a moved push target, `gh`
//! becoming unusable, and the feature being switched off.

use daruda_github::RepoPrs;
use daruda_github::link::pick;
use daruda_github::remote_parse::parse_remote;
use daruda_github::{PrSummary, RepoId};
use daruda_store::project::LaneRef;
use gpui::Context;

use super::lane_link::LaneLink;
use crate::github::{PrDemand, PrLink, PrLookup, PrSubject, PrUpdate, is_unusable};
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

/// What an answer says about a lane asking about `subject`; `None` when the
/// answer does not cover it (asked before the lane wanted it). A review
/// lane's PR is never looked up by branch, so it never reads as having none.
fn lookup_subject(repo: &RepoId, subject: &PrSubject, prs: &RepoPrs) -> Option<PrLookup> {
    match subject {
        PrSubject::Branch(branch) => {
            let candidates = prs.by_branch.get(branch)?;
            Some(lookup_for(
                repo,
                branch,
                prs.default_branch.as_deref(),
                candidates,
            ))
        }
        PrSubject::Number(number) => Some(match prs.by_number.get(number)? {
            Some(pr) => PrLookup::Linked(Box::new(PrLink {
                repo: repo.clone(),
                pr: pr.clone(),
            })),
            None => PrLookup::Unknown,
        }),
    }
}

/// What a lane needs after its links change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum AfterEdit {
    /// Only its mirror: what changed is a record, not which pull request.
    Mirror,
    /// Its pull request asked again: the user changed which one it is.
    Relink,
}

impl LaneScoped {
    /// Where the lane's branch pushes, with the GitHub repository its remote
    /// names; `None` without a push target or for a remote that is not one.
    pub(in crate::workspace) fn github_push(&self) -> Option<(RepoId, &PushTarget)> {
        let push = self.git.push_target.as_ref()?;
        Some((parse_remote(&push.url)?, push))
    }

    /// How the lane is tied to its pull request.
    pub(in crate::workspace) fn lane_link(&self) -> Option<LaneLink> {
        LaneLink::resolve(self.git.push_target.as_ref(), self.pr_links.as_ref())
    }

    /// Which pull request the lane works on, and in which repository.
    pub(in crate::workspace) fn pr_subject(&self) -> Option<(RepoId, PrSubject)> {
        self.lane_link()?.subject()
    }

    /// Why the lane is tied to its pull request, once it is.
    pub(in crate::workspace) fn link_source(&self) -> Option<crate::github::links::Source> {
        self.lane_link()?.source(self.pr.link().is_some())
    }

    /// Whether the user took `repo#number` away from this branch.
    pub(in crate::workspace) fn suppresses(&self, repo: &RepoId, number: u64) -> bool {
        self.pr_links.as_ref().is_some_and(|links| {
            links
                .suppressed
                .iter()
                .any(|pr| crate::github::links::pr_key(pr) == (repo.clone(), number))
        })
    }

    /// The pull request the lane works on: one named by number is known
    /// before GitHub answers; one found by branch name once it is linked.
    pub(in crate::workspace) fn pr_key(&self) -> Option<(RepoId, u64)> {
        self.lane_link()
            .and_then(|link| link.known_key())
            .or_else(|| {
                self.pr
                    .link()
                    .map(|link| (link.repo.clone(), link.pr.number))
            })
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

    /// Every lane with a pull request question for a GitHub-shaped repository.
    pub(crate) fn pr_demand(&self) -> Vec<PrDemand> {
        self.lane_scoped
            .iter()
            .filter_map(|(target, state)| {
                let (repo, subject) = state.pr_subject()?;
                Some(PrDemand {
                    repo,
                    subject,
                    focused: *target == self.active,
                })
            })
            .collect()
    }

    /// Apply a poll answer for `repo` to every lane asking about it. A lane
    /// whose question the answer does not cover (its branch moved while the
    /// request ran) keeps what it had until the next answer that does.
    fn apply_repo_prs(&mut self, repo: &RepoId, prs: &RepoPrs, cx: &mut Context<Self>) {
        let mut changed = false;
        let mut found = Vec::new();
        for (lane, state) in self.lane_scoped.iter_mut() {
            let Some((lane_repo, subject)) = state.pr_subject() else {
                continue;
            };
            if lane_repo != *repo {
                continue;
            }
            let Some(mut lookup) = lookup_subject(repo, &subject, prs) else {
                continue;
            };
            // One the user took away from the branch is not shown as its
            // pull request. Still open, it is offered to tie again; closed
            // or merged, the branch has none to work on and may create one.
            if matches!(subject, PrSubject::Branch(_))
                && let PrLookup::Linked(link) = &lookup
                && state.suppresses(&link.repo, link.pr.number)
            {
                lookup = if link.pr.state.is_active() {
                    PrLookup::Untied(link.pr.number)
                } else {
                    PrLookup::NoPr
                };
            }
            let newly = match &lookup {
                PrLookup::Linked(link)
                    if state.pr.link().map(|held| held.pr.number) != Some(link.pr.number) =>
                {
                    Some((link.repo.clone(), link.pr.number))
                }
                _ => None,
            };
            if state.pr.replace(lookup) {
                changed = true;
                if let (Some(pr), Some(source)) = (newly, state.link_source()) {
                    found.push((*lane, pr, source));
                }
            }
        }
        // Each new link enters the worktree's history.
        for (lane, (repo, number), source) in found {
            self.record_link(lane, &repo, number, source, cx);
        }
        if changed {
            cx.notify();
        }
    }

    /// Re-read every git lane's push target: the feature was switched on
    /// (they are not kept current while it is off), or the Pull requests
    /// page needs every worktree's pull request, panes opened or not.
    pub(in crate::workspace) fn refresh_push_targets(&mut self, cx: &mut Context<Self>) {
        let lanes: Vec<LaneRef> = self.projects.lanes().map(|(lane, _, _)| lane).collect();
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
                .pr_subject()
                .is_some_and(|(repo, _)| repo.host() == host);
            if on_host {
                changed |= state.pr.replace(PrLookup::Unknown);
            }
        }
        if changed {
            cx.notify();
        }
    }

    /// Record where `target` pushes. A different repository, branch or review
    /// mark drops the old answer and asks the poller about the new one at once.
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
            (Some(old), Some(new)) => {
                old.url != new.url || old.branch != new.branch || old.review != new.review
            }
            _ => true,
        };
        state.git.push_target = found;
        if moved {
            state.pr.replace(PrLookup::Unknown);
            self.sync_lane_links(target, cx);
            if let Some((repo, _)) = self
                .lane_scoped
                .get(&target)
                .and_then(LaneScoped::pr_subject)
            {
                crate::github::PrPoller::nudge(repo, cx);
            }
        }
        true
    }

    /// Read what `LinkStore` holds for the lane's worktree and branch into
    /// the lane: the one place the mirror is written, so a push target read,
    /// a link chosen or taken away and a restart all agree.
    pub(in crate::workspace) fn sync_lane_links(&mut self, lane: LaneRef, cx: &mut Context<Self>) {
        let path = self.git_lane_path(lane);
        let Some(state) = self.lane_scoped.get_mut(&lane) else {
            return;
        };
        let links = match (path, state.git.push_target.as_ref()) {
            (Some(path), Some(push)) => crate::github::LinkStore::of(&path, &push.branch, cx),
            _ => None,
        };
        state.pr_links = links;
    }

    /// The lane's links changed: mirror them and ask GitHub again at once.
    pub(in crate::workspace) fn relink_lane(&mut self, lane: LaneRef, cx: &mut Context<Self>) {
        self.sync_lane_links(lane, cx);
        if let Some(state) = self.lane_scoped.get_mut(&lane) {
            state.pr.replace(PrLookup::Unknown);
            if let Some((repo, _)) = state.pr_subject() {
                crate::github::PrPoller::nudge(repo, cx);
            }
        }
        cx.notify();
    }

    /// Enter `repo#number` in the lane's worktree history as linked now.
    fn record_link(
        &mut self,
        lane: LaneRef,
        repo: &RepoId,
        number: u64,
        source: crate::github::links::Source,
        cx: &mut Context<Self>,
    ) {
        let pr = crate::github::links::pr_ref(repo, number);
        self.edit_lane_links(lane, AfterEdit::Mirror, cx, |links, path, branch, now| {
            links.found(path, branch, pr, source, now)
        });
    }

    /// Change what is kept for `lane`'s worktree and branch, then bring the
    /// lane up to date: its mirror of the links, and with
    /// [`AfterEdit::Relink`] its pull request too. Every change to the links
    /// goes through here, so none leaves a lane showing what it replaced.
    pub(in crate::workspace) fn edit_lane_links(
        &mut self,
        lane: LaneRef,
        after: AfterEdit,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut daruda_store::pr_links::PrLinks, &std::path::Path, &str, i64),
    ) {
        let Some((path, branch)) = self.lane_worktree_branch(lane) else {
            return;
        };
        let now = chrono::Utc::now().timestamp();
        crate::github::LinkStore::edit(cx, |links| edit(links, &path, &branch, now));
        match after {
            AfterEdit::Mirror => self.sync_lane_links(lane, cx),
            AfterEdit::Relink => self.relink_lane(lane, cx),
        }
    }

    /// The lane's worktree and the branch it pushes, the store's key.
    pub(in crate::workspace) fn lane_worktree_branch(
        &self,
        lane: LaneRef,
    ) -> Option<(std::path::PathBuf, String)> {
        let path = self.git_lane_path(lane)?;
        let branch = self
            .lane_scoped
            .get(&lane)?
            .git
            .push_target
            .as_ref()?
            .branch
            .clone();
        Some((path, branch))
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

    #[test]
    fn a_review_lane_reads_its_numbers_answer_and_never_none() {
        let mut prs = RepoPrs::default();
        let fork = PrSummary {
            number: 9,
            cross_repo: true,
            ..pr("main")
        };
        prs.by_number.insert(9, Some(fork.clone()));
        prs.by_number.insert(4, None);
        // (label, number, expected)
        let cases = [
            (
                "answered",
                9,
                Some(PrLookup::Linked(Box::new(PrLink {
                    repo: repo(),
                    pr: fork,
                }))),
            ),
            (
                "no such PR: not known, never NoPr",
                4,
                Some(PrLookup::Unknown),
            ),
            ("not asked in this answer", 5, None),
        ];
        for (label, number, expected) in cases {
            assert_eq!(
                lookup_subject(&repo(), &PrSubject::Number(number), &prs),
                expected,
                "{label}"
            );
        }
        assert_eq!(
            lookup_subject(&repo(), &PrSubject::Branch("x".into()), &prs),
            None,
            "a branch the answer did not cover"
        );
    }
}
