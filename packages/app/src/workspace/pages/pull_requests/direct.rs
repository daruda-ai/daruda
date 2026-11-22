//! Opening a pull request by a pasted reference: a URL, `owner/name#n` or a
//! number the context places. Its detail is read first, then shown on the
//! Pull requests page with the tab the reference points at. Nothing about
//! worktrees changes: no worktree is made and no link is tied; the detail
//! offers that separately.

use daruda_github::address::{PrAddress, PrTarget, parse_pr_address};
use daruda_github::{GhError, PrDetail, PrListItem, RepoId};
use daruda_store::observability::error_report::{ErrorReport, ErrorSeverity};
use gpui::{Context, Window};

use super::detail::{PrOpening, PrTab};
use crate::gh_host::GhHost;
use crate::surface::strings::pull_requests as s;
use crate::workspace::Workspace;

/// Where a pull request opened by its address is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum PrDestination {
    /// Over the Pull requests page's list.
    Page,
    /// In this worktree's side tabs.
    Beside(daruda_store::project::LaneRef),
}

impl Workspace {
    /// The palette's Open pull request: ask for a reference, then open it.
    pub(in crate::workspace) fn on_open_pull_request(
        &mut self,
        _: &crate::workspace::OpenPullRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::workspace::dialog_helpers::open_validated_single_field_dialog(
            cx.entity().downgrade(),
            s::open_pr_title(),
            s::open_pr_placeholder(),
            None,
            |ws, value, window, cx| {
                let Some(text) = value else {
                    return Ok(());
                };
                let (repo, address) = ws.resolve_pr_address(&text, cx)?;
                ws.open_pr_by_address(repo, address, PrDestination::Page, window, cx);
                Ok(())
            },
            window,
            cx,
        );
    }

    /// The repository `text` names a pull request in, from the reference
    /// itself or, for a bare number, from where the user is. `Err` says why
    /// it cannot be placed.
    pub(in crate::workspace) fn resolve_pr_address(
        &self,
        text: &str,
        cx: &gpui::App,
    ) -> Result<(RepoId, PrAddress), String> {
        // Off, nothing asks `gh`.
        if !crate::github::enabled(cx) {
            return Err(s::open_pr_github_off());
        }
        let address = parse_pr_address(text).ok_or_else(s::open_pr_unreadable)?;
        let known: Vec<RepoId> = {
            let mut repos: Vec<RepoId> = self
                .lane_scoped
                .values()
                .filter_map(|state| state.github_push().map(|(repo, _)| repo))
                .collect();
            repos.sort_by_key(RepoId::qualified);
            repos.dedup();
            repos
        };
        let active = self
            .lane_scoped
            .get(&self.active)
            .and_then(|state| state.github_push().map(|(repo, _)| repo));
        let repo = match &address.repo {
            // A URL names its host: kept, whatever the remotes say.
            Some(repo) if !address.host_unknown => repo.clone(),
            // `owner/name`: the shown worktree's host when it is on that
            // repository, else the one host a worktree has it on, else
            // github.com; on several hosts, asked rather than guessed.
            Some(repo) => {
                let same =
                    |held: &RepoId| held.owner() == repo.owner() && held.name() == repo.name();
                match active.as_ref().filter(|held| same(held)) {
                    Some(held) => held.clone(),
                    None => {
                        let hosts: Vec<&RepoId> = known.iter().filter(|held| same(held)).collect();
                        match hosts.as_slice() {
                            [] => repo.clone(),
                            [only] => (*only).clone(),
                            _ => return Err(s::open_pr_which_repository()),
                        }
                    }
                }
            }
            // A number: the shown worktree's repository, else the only one.
            None => match active {
                Some(repo) => repo,
                None => match known.as_slice() {
                    [only] => only.clone(),
                    [] => return Err(s::open_pr_no_repository()),
                    _ => return Err(s::open_pr_which_repository()),
                },
            },
        };
        Ok((repo, address))
    }

    /// `repo#number`, shown at `to`: what an entry that names its pull
    /// request when it is drawn opens, rather than look one up when pressed.
    pub(in crate::workspace) fn open_pr_at(
        &mut self,
        repo: RepoId,
        number: u64,
        to: PrDestination,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let address = PrAddress {
            repo: Some(repo.clone()),
            host_unknown: false,
            number,
            target: None,
        };
        self.open_pr_by_address(repo, address, to, window, cx);
    }

    /// `repo#number`, the pull request the worktree on screen works on,
    /// opened from its Git panel: beside its tabs, where files open there,
    /// else on the page.
    pub(in crate::workspace) fn open_lane_pr(
        &mut self,
        repo: RepoId,
        number: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The rule a file follows, but for a tab only the side tabs hold;
        // the click enters what it opens.
        let entry = crate::workspace::main_area::tab_ops::PaneEntry::Enter;
        let to = if self.group_facts().side_only_goes_beside(entry) {
            PrDestination::Beside(self.active)
        } else {
            PrDestination::Page
        };
        self.open_pr_at(repo, number, to, window, cx);
    }

    /// Another pull request of the stack detail `from` shows, opened where
    /// `from` is being read: in the worktree's pane it takes the pane, on
    /// the page it takes the detail's place.
    pub(in crate::workspace) fn open_stack_entry(
        &mut self,
        from: super::detail::PrDetailId,
        repo: RepoId,
        number: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let to = if self.shown_pr_pane_detail() == Some(from) {
            PrDestination::Beside(self.active)
        } else {
            PrDestination::Page
        };
        self.open_pr_at(repo, number, to, window, cx);
    }

    /// Read `repo#number`, then show it at `to` with the tab `address`
    /// points at.
    pub(in crate::workspace) fn open_pr_by_address(
        &mut self,
        repo: RepoId,
        address: PrAddress,
        to: PrDestination,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Off, nothing asks `gh`; said, since a click asked for it.
        if !crate::github::enabled(cx) {
            let report = ErrorReport::new(s::open_pr_github_off())
                .severity(ErrorSeverity::Warning)
                .at(file!(), line!())
                .dedup("github.open_pr")
                .build();
            self.report_error(report, cx);
            return;
        }
        let gh = GhHost::global(cx);
        let read_repo = repo.clone();
        let number = address.number;
        cx.spawn_in(window, async move |ws, cx| {
            let read = cx
                .background_executor()
                .spawn(async move { gh.pr_detail(&read_repo, number) })
                .await;
            // SILENT-OK: the window closed while GitHub answered.
            let _ = ws.update_in(cx, |ws, window, cx| {
                ws.land_pr_by_address(repo, address, to, read, window, cx)
            });
        })
        .detach();
    }

    fn land_pr_by_address(
        &mut self,
        repo: RepoId,
        address: PrAddress,
        to: PrDestination,
        read: Result<PrDetail, GhError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let detail = match read {
            Ok(detail) => detail,
            Err(e) => {
                let report = ErrorReport::new(s::open_pr_failed(format!(
                    "{}#{}",
                    repo.qualified(),
                    address.number
                )))
                .severity(ErrorSeverity::Warning)
                .from_error(&e)
                .at(file!(), line!())
                .dedup("github.open_pr")
                .build();
                self.report_error(report, cx);
                return;
            }
        };
        let item = listed(repo, &detail);
        // Comment anchors open their tab; scrolling to the comment itself
        // is not done yet, and Commits has no tab of its own.
        let tab = match address.target {
            Some(PrTarget::Files) => Some(PrTab::Files),
            Some(PrTarget::Checks) => Some(PrTab::Checks),
            Some(PrTarget::ReviewComment(_)) => Some(PrTab::Threads),
            Some(PrTarget::Comment(_)) => Some(PrTab::Description),
            Some(PrTarget::Commits) | None => None,
        };
        let opening = PrOpening {
            tab,
            read: Some(Box::new(detail)),
        };
        match to {
            PrDestination::Page => {
                // Asked for from anywhere: the page comes up to show it.
                self.show_page(super::Page::PullRequests, cx);
                self.open_pr_detail_as(item, opening, window, cx)
            }
            PrDestination::Beside(lane) => self.open_pr_pane(lane, item, opening, window, cx),
        }
    }
}

/// `detail` as a row of the list would have it, for a detail opened
/// without one.
pub(super) fn listed(repo: RepoId, detail: &PrDetail) -> PrListItem {
    PrListItem {
        repo,
        author: detail.author.clone(),
        head_owner: None,
        base_branch: detail.base_branch.clone(),
        comments: 0,
        pr: detail.summary.clone(),
    }
}

#[cfg(test)]
mod tests;
