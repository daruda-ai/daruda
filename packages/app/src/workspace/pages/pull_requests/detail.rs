//! The Pull requests page's detail: one pull request's checks, review
//! threads and description, shown over the list. It holds no edits, so
//! leaving it never asks. Reads run on the background executor and land
//! only while the detail that asked is still the one shown.

use std::collections::BTreeMap;

use daruda_github::{ActionsJob, GhError, PrDetail, PrListItem, ReviewThreads};
use daruda_store::observability::error_report::{ErrorReport, ErrorSeverity};
use gpui::{Context, Window};

use super::actions::PrAction;
use super::list::{self, LaneFacts};
use crate::gh_host::GhHost;
use crate::github::PrLink;
use crate::surface::strings::error;
use crate::workspace::Workspace;
use crate::workspace::github_ops::review_send::PrSendTarget;
use crate::workspace::pages::Page;
use crate::workspace::spawn_helpers::spawn_bg_work_and_mutate;

/// Names the page's detail. Never reused, so a read that lands after its
/// detail was replaced or closed finds nothing to fill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(in crate::workspace) struct PrDetailId(pub u64);

/// Something read from GitHub for the detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) enum Load<T> {
    Loading,
    Ready(T),
    /// What `gh` said.
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(in crate::workspace) enum PrTab {
    #[default]
    Checks,
    Threads,
    Description,
}

impl PrTab {
    pub(in crate::workspace) const ALL: [Self; 3] =
        [Self::Checks, Self::Threads, Self::Description];
}

/// A failed Actions job's log, keyed as `(run, job)`.
type JobKey = (u64, u64);

fn job_key(job: ActionsJob) -> JobKey {
    (job.run, job.job)
}

pub(in crate::workspace) struct PrDetailState {
    pub id: PrDetailId,
    /// The row it was opened from: what the header shows until the detail
    /// is read, and how its worktree is found.
    pub item: PrListItem,
    pub load: Load<PrDetail>,
    pub tab: PrTab,
    /// The unresolved threads, read when their tab is first shown.
    pub threads: Option<Load<ReviewThreads>>,
    /// The failed Actions jobs whose log is open.
    pub(super) logs: BTreeMap<JobKey, Load<String>>,
    /// What it is doing to the pull request on GitHub.
    pub(super) action: PrAction,
    /// Which read of the detail may land; a refresh outdates the one before.
    read: u64,
}

impl PrDetailState {
    /// A detail for `item`, its pull request not read yet.
    pub(super) fn new(id: PrDetailId, item: PrListItem) -> Self {
        Self {
            id,
            item,
            load: Load::Loading,
            tab: PrTab::default(),
            threads: None,
            logs: BTreeMap::new(),
            action: PrAction::Idle,
            read: 0,
        }
    }

    pub(in crate::workspace) fn log(&self, job: ActionsJob) -> Option<&Load<String>> {
        self.logs.get(&job_key(job))
    }
}

impl Workspace {
    pub(in crate::workspace) fn pr_detail_id(&self) -> Option<PrDetailId> {
        self.pages.pull_requests.detail.as_ref().map(|d| d.id)
    }

    pub(super) fn pr_detail_mut(&mut self, id: PrDetailId) -> Option<&mut PrDetailState> {
        self.pages
            .pull_requests
            .detail
            .as_mut()
            .filter(|detail| detail.id == id)
    }

    /// Show `item` over the list. The same pull request already open is
    /// shown as it is; another replaces the detail.
    pub(in crate::workspace) fn open_pr_detail(
        &mut self,
        item: PrListItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let open = self
            .pages
            .pull_requests
            .detail
            .as_ref()
            .is_some_and(|d| d.item.repo == item.repo && d.item.pr.number == item.pr.number);
        if open {
            self.show_page(Page::PullRequests, cx);
            return;
        }
        self.leave_page_detail_then(Page::PullRequests, window, cx, move |ws, _, cx| {
            ws.install_pr_detail(item, cx);
        });
    }

    fn install_pr_detail(&mut self, item: PrListItem, cx: &mut Context<Self>) {
        let id = PrDetailId(self.alloc_id());
        self.pages.pull_requests.detail = Some(PrDetailState::new(id, item));
        self.read_pr_detail(id, cx);
        cx.notify();
    }

    /// Back to the list.
    pub(in crate::workspace) fn back_to_pr_list(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.leave_page_detail_then(Page::PullRequests, window, cx, |_, _, _| {});
    }

    /// Read the detail and the list again: the detail's refresh button.
    pub(in crate::workspace) fn refresh_pr_detail(
        &mut self,
        id: PrDetailId,
        cx: &mut Context<Self>,
    ) {
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        detail.logs.clear();
        let reread_threads = detail.threads.take().is_some() && detail.tab == PrTab::Threads;
        self.read_pr_detail(id, cx);
        if reread_threads {
            self.read_pr_threads(id, cx);
        }
        self.refresh_pr_list(cx);
        cx.notify();
    }

    fn read_pr_detail(&mut self, id: PrDetailId, cx: &mut Context<Self>) {
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        detail.read += 1;
        let read = detail.read;
        let (repo, number) = (detail.item.repo.clone(), detail.item.pr.number);
        let gh = GhHost::global(cx);
        spawn_bg_work_and_mutate(
            cx,
            move || gh.pr_detail(&repo, number),
            move |ws, result, cx| ws.land_pr_detail(id, read, result, cx),
        )
        .detach();
    }

    /// A refresh that fails keeps what an earlier read showed.
    pub(in crate::workspace) fn land_pr_detail(
        &mut self,
        id: PrDetailId,
        read: u64,
        result: Result<PrDetail, GhError>,
        cx: &mut Context<Self>,
    ) {
        let Some(detail) = self.pr_detail_mut(id).filter(|d| d.read == read) else {
            return;
        };
        match result {
            Ok(loaded) => detail.load = Load::Ready(loaded),
            Err(e) => {
                if !matches!(detail.load, Load::Ready(_)) {
                    detail.load = Load::Failed(e.to_string());
                }
                self.report_pr_read(&e, "github.pr_detail.read", cx);
            }
        }
        cx.notify();
    }

    /// Show `tab`; the threads are read the first time theirs is shown, and
    /// again after a failed read.
    pub(in crate::workspace) fn select_pr_detail_tab(
        &mut self,
        id: PrDetailId,
        tab: PrTab,
        cx: &mut Context<Self>,
    ) {
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        detail.tab = tab;
        let unread = matches!(detail.threads, None | Some(Load::Failed(_)));
        if tab == PrTab::Threads && unread {
            self.read_pr_threads(id, cx);
        }
        cx.notify();
    }

    fn read_pr_threads(&mut self, id: PrDetailId, cx: &mut Context<Self>) {
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        detail.threads = Some(Load::Loading);
        let (repo, number) = (detail.item.repo.clone(), detail.item.pr.number);
        let gh = GhHost::global(cx);
        spawn_bg_work_and_mutate(
            cx,
            move || gh.review_threads(&repo, number),
            move |ws, result, cx| ws.land_pr_threads(id, result, cx),
        )
        .detach();
    }

    fn land_pr_threads(
        &mut self,
        id: PrDetailId,
        result: Result<ReviewThreads, GhError>,
        cx: &mut Context<Self>,
    ) {
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        // A refresh dropped the read this answers.
        if detail.threads != Some(Load::Loading) {
            return;
        }
        detail.threads = Some(match &result {
            Ok(threads) => Load::Ready(threads.clone()),
            Err(e) => Load::Failed(e.to_string()),
        });
        if let Err(e) = result {
            self.report_pr_read(&e, "github.pr_threads.read", cx);
        }
        cx.notify();
    }

    /// Open a failed Actions job's log under its check, reading it the
    /// first time; close it if open.
    pub(in crate::workspace) fn toggle_pr_check_log(
        &mut self,
        id: PrDetailId,
        job: ActionsJob,
        cx: &mut Context<Self>,
    ) {
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        if detail.logs.remove(&job_key(job)).is_some() {
            cx.notify();
            return;
        }
        detail.logs.insert(job_key(job), Load::Loading);
        let repo = detail.item.repo.clone();
        let gh = GhHost::global(cx);
        spawn_bg_work_and_mutate(
            cx,
            move || {
                gh.failed_log(&repo, job)
                    .map(|raw| daruda_github::prompt::clean_log(&raw))
            },
            move |ws, result, cx| ws.land_pr_check_log(id, job, result, cx),
        )
        .detach();
        cx.notify();
    }

    fn land_pr_check_log(
        &mut self,
        id: PrDetailId,
        job: ActionsJob,
        result: Result<String, GhError>,
        cx: &mut Context<Self>,
    ) {
        let Some(slot) = self
            .pr_detail_mut(id)
            .and_then(|detail| detail.logs.get_mut(&job_key(job)))
            .filter(|slot| **slot == Load::Loading)
        else {
            return;
        };
        *slot = match &result {
            Ok(log) => Load::Ready(log.clone()),
            Err(e) => Load::Failed(e.to_string()),
        };
        if let Err(e) = result {
            self.report_pr_read(&e, "github.pr_log.read", cx);
        }
        cx.notify();
    }

    /// The worktree working on the shown pull request.
    pub(in crate::workspace) fn pr_detail_lane(&self, item: &PrListItem) -> Option<LaneFacts> {
        list::linked_lane(item, &self.pr_page_lanes()).cloned()
    }

    /// Where the detail's Send to agent goes: the linked worktree's chat,
    /// with the pull request as last read. `None` without either.
    fn pr_detail_send_target(&self, id: PrDetailId) -> Option<PrSendTarget> {
        let detail = self
            .pages
            .pull_requests
            .detail
            .as_ref()
            .filter(|d| d.id == id)?;
        let Load::Ready(loaded) = &detail.load else {
            return None;
        };
        let lane = self.pr_detail_lane(&detail.item)?.lane;
        Some(PrSendTarget {
            lane,
            link: PrLink {
                repo: detail.item.repo.clone(),
                pr: loaded.summary.clone(),
            },
        })
    }

    pub(in crate::workspace) fn send_pr_detail_checks(
        &mut self,
        id: PrDetailId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(target) = self.pr_detail_send_target(id) {
            self.send_failed_checks_to(target, window, cx);
        }
    }

    pub(in crate::workspace) fn send_pr_detail_threads(
        &mut self,
        id: PrDetailId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(target) = self.pr_detail_send_target(id) {
            self.send_review_comments_to(target, window, cx);
        }
    }

    /// Go to the worktree working on the shown pull request.
    pub(in crate::workspace) fn open_pr_detail_lane(
        &mut self,
        id: PrDetailId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let lane = self
            .pages
            .pull_requests
            .detail
            .as_ref()
            .filter(|d| d.id == id)
            .and_then(|d| self.pr_detail_lane(&d.item));
        if let Some(lane) = lane {
            self.activate_lane(lane.lane, window, cx);
        }
    }

    fn report_pr_read(&mut self, e: &GhError, dedup: &'static str, cx: &mut Context<Self>) {
        let report = ErrorReport::new(error::github_read_failed())
            .severity(ErrorSeverity::Warning)
            .from_error(e)
            .at(file!(), line!())
            .dedup(dedup)
            .build();
        self.report_error(report, cx);
    }
}
