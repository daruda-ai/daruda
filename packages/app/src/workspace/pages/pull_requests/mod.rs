//! The Pull requests page: the open pull requests of every GitHub repository
//! this window's worktrees push to, grouped by repository. The list itself
//! is kept by [`crate::workspace::github_ops::pr_list`]; this page decides
//! when it is searched (only while shown) and stages what it draws.

use daruda_github::PrFilter;
use gpui::{App, Context, WeakEntity};

use super::Page;
use crate::github::PrPoller;
use crate::workspace::Workspace;

mod lane;
pub(in crate::workspace) mod list;
pub(in crate::workspace) mod render;

use std::collections::BTreeSet;

use list::{LaneFacts, PageInputs, PrKey, PrPageList};

/// What the page remembers while it is not on screen. Never persisted.
pub(in crate::workspace) struct PullRequestsPage {
    /// The filter the page shows, and shows again when reopened.
    filter: PrFilter,
    /// Pull requests a worktree is being made for, so their rows do not
    /// offer to make a second one.
    creating: BTreeSet<PrKey>,
    /// Staged each frame while the page is on screen; `None` otherwise.
    pub(in crate::workspace) snapshot: Option<PrPageSnapshot>,
}

impl Default for PullRequestsPage {
    fn default() -> Self {
        Self {
            filter: PrFilter::Mine,
            creating: BTreeSet::new(),
            snapshot: None,
        }
    }
}

/// Plain data for one frame of the page, so drawing it reads no entity.
pub(in crate::workspace) struct PrPageSnapshot {
    pub workspace: WeakEntity<Workspace>,
    pub list: PrPageList,
}

impl Workspace {
    /// Search for the list while the page is on screen and stop when it
    /// leaves. Called wherever the shown page changes.
    pub(in crate::workspace) fn follow_pr_page(&mut self, cx: &mut Context<Self>) {
        if self.active_page() == Some(Page::PullRequests) {
            self.show_pr_list(self.pages.pull_requests.filter, cx);
        } else {
            self.hide_pr_list(cx);
        }
    }

    /// Show `filter`'s pull requests, now and whenever the page reopens.
    pub(in crate::workspace) fn set_pr_page_filter(
        &mut self,
        filter: PrFilter,
        cx: &mut Context<Self>,
    ) {
        if self.pages.pull_requests.filter == filter {
            return;
        }
        self.pages.pull_requests.filter = filter;
        self.follow_pr_page(cx);
        cx.notify();
    }

    /// Stage what the page draws this frame. Only while it is on screen:
    /// the rows cost a pass over every lane and pull request.
    pub(in crate::workspace) fn stage_pr_page(&mut self, cx: &Context<Self>) {
        let shown = self.active_page() == Some(Page::PullRequests);
        self.pages.pull_requests.snapshot = shown.then(|| PrPageSnapshot {
            workspace: cx.weak_entity(),
            list: self.pr_page_list(cx),
        });
    }

    pub(in crate::workspace) fn pr_page_list(&self, cx: &App) -> PrPageList {
        let repos = self.github_repos();
        let lanes = self.pr_page_lanes();
        let inputs = PageInputs {
            enabled: crate::github::enabled(cx),
            filter: self.pages.pull_requests.filter,
            repos: &repos,
            list: self.pr_list(),
            lanes: &lanes,
            creating: &self.pages.pull_requests.creating,
            now: chrono::Utc::now(),
        };
        list::page(&inputs, |host| PrPoller::availability(host, cx))
    }

    /// Every lane, with what matching it to a pull request reads.
    fn pr_page_lanes(&self) -> Vec<LaneFacts> {
        self.projects
            .lanes()
            .map(|(lane, _, record)| {
                let state = self.lane_scoped.get(&lane);
                LaneFacts {
                    lane,
                    name: record.display_name(),
                    repo: state
                        .and_then(|state| state.github_push())
                        .map(|(repo, _)| repo),
                    branch: state
                        .and_then(|state| state.git.tracking.as_ref())
                        .and_then(|tracking| tracking.branch.clone()),
                    pr: state
                        .and_then(|state| state.pr.link())
                        .map(|link| (link.repo.clone(), link.pr.number)),
                }
            })
            .collect()
    }

    /// How many open pull requests this window's worktrees are on, for the
    /// page's entry in the left dock. Known without a search.
    pub(in crate::workspace) fn open_lane_pr_count(&self) -> usize {
        let prs: std::collections::BTreeSet<_> = self
            .lane_scoped
            .values()
            .filter_map(|state| state.pr.link())
            .filter(|link| link.pr.state.is_active())
            .map(|link| (link.repo.clone(), link.pr.number))
            .collect();
        prs.len()
    }
}

#[cfg(test)]
mod tests;
