//! Pull-request fixtures for captures. Every capture starts on an empty
//! `FakeGh` (`startup::inspection::before_globals`); these scenarios replace
//! it with one that answers like the seeds, so the running poller does not
//! overwrite what was seeded.

use std::sync::Arc;

use daruda_github::availability::{GhAuth, GhAvailability};
use daruda_github::fake::FakeGh;
use daruda_github::{
    ActionsJob, BlockReason, CheckEntry, CheckKind, CheckRollup, CheckRun, CheckStatus,
    FailedCheck, GITHUB_HOST, MergeMethod, Mergeability, PrDetail, PrFilter, PrListItem, PrState,
    PrSummary, RepoId, ReviewDecision, ReviewerState,
};
use daruda_store::project::LaneRef;
use gpui::{Context, Window};

use crate::gh_host::GhHost;
use crate::github::{PrLink, PrLookup, PrPoller};
use crate::lane::git::{GitTracking, PushTarget};
use crate::workspace::Workspace;
use crate::workspace::pages::Page;
use crate::workspace::pages::pull_requests::detail::Load;

fn repo() -> RepoId {
    RepoId::new(GITHUB_HOST, "daruda", "daruda")
}

fn pr(number: u64, head: &str, title: &str, state: PrState, checks: CheckRollup) -> PrSummary {
    let failed = match checks {
        CheckRollup::Failing { failed } => (0..failed.unwrap_or(0))
            .map(|i| FailedCheck {
                workflow: None,
                event: None,
                name: format!("build ({i})"),
                details_url: None,
                job: None,
            })
            .collect(),
        _ => Vec::new(),
    };
    PrSummary {
        number,
        url: format!("https://github.com/daruda/daruda/pull/{number}"),
        title: title.into(),
        state,
        head_branch: head.into(),
        cross_repo: false,
        checks,
        failed,
        review: None,
        updated_at: String::new(),
    }
}

/// What a capture of the pull request detail shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum PrDetailShot {
    /// The first failing check's log open.
    FailedLog,
    /// A mergeable pull request, its squash-merge confirmation up.
    MergeConfirm,
}

/// What the active lane's PR looks like in a capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum PrShot {
    /// A PR whose checks fail.
    Linked,
    /// GitHub answers; the branch has no PR yet.
    NoPr,
    /// `gh` is not installed.
    GhMissing,
}

/// Answer for GitHub the way the seeds say it is.
fn install_fake_gh(auth: GhAuth, prs: Vec<PrSummary>, cx: &mut gpui::App) {
    let gh = FakeGh::new();
    gh.set_auth(GITHUB_HOST, auth);
    gh.set_prs(&repo(), prs);
    GhHost::set_global(Arc::new(gh), cx);
}

impl Workspace {
    /// Written directly rather than through `set_push_target`: that would
    /// nudge the poller before the scripted answers are installed.
    fn seed_push_target_for_shot(&mut self, target: LaneRef, branch: &str) {
        self.seed_push_to_for_shot(target, "git@github.com:daruda/daruda.git", branch);
    }

    fn seed_push_to_for_shot(&mut self, target: LaneRef, url: &str, branch: &str) {
        self.lane_scoped_mut(target).git.push_target = Some(PushTarget {
            remote: "origin".into(),
            url: url.into(),
            branch: branch.into(),
            published: true,
            has_upstream: false,
        });
    }

    /// The active lane pushing to GitHub, its PR as `shot` says.
    pub(in crate::workspace) fn seed_pr_for_shot(&mut self, shot: PrShot, cx: &mut Context<Self>) {
        const BRANCH: &str = "sidebar-sizing";
        let active = self.active;
        self.seed_push_target_for_shot(active, BRANCH);
        let linked = pr(
            482,
            BRANCH,
            "Adjust sidebar presentation for narrow docks",
            PrState::Open,
            CheckRollup::Failing { failed: Some(2) },
        );
        let (auth, availability, prs) = match shot {
            PrShot::Linked => (GhAuth::Ready, GhAvailability::Ready, vec![linked.clone()]),
            PrShot::NoPr => (GhAuth::Ready, GhAvailability::Ready, Vec::new()),
            PrShot::GhMissing => (GhAuth::Missing, GhAvailability::Missing, Vec::new()),
        };
        install_fake_gh(auth, prs, cx);
        PrPoller::seed_for_shot(GITHUB_HOST, availability, cx);
        let lookup = match shot {
            PrShot::Linked => PrLookup::Linked(Box::new(PrLink {
                repo: repo(),
                pr: linked,
            })),
            PrShot::NoPr => PrLookup::NoPr,
            PrShot::GhMissing => PrLookup::Unknown,
        };
        self.seed_pr_lookup_for_shot(active, lookup, cx);
        cx.notify();
    }

    /// One PR per state on the active project's lanes, for the worktree list.
    pub(in crate::workspace) fn seed_lane_prs_for_shot(&mut self, cx: &mut Context<Self>) {
        let project = self.active.project;
        let lanes: Vec<_> = self
            .project_for(project)
            .map(|p| p.lanes.iter().map(|lane| lane.id).collect())
            .unwrap_or_default();
        let states = [
            (PrState::Open, CheckRollup::Failing { failed: Some(1) }),
            (PrState::Draft, CheckRollup::Pending),
            (PrState::Open, CheckRollup::Passing),
            (PrState::Merged, CheckRollup::Passing),
        ];
        let mut prs = Vec::new();
        for (i, (lane, (state, checks))) in lanes.into_iter().zip(states).enumerate() {
            let target = LaneRef { project, lane };
            // One branch per lane, so one answer links each to its own PR.
            let branch = format!("lane-{i}");
            self.seed_push_target_for_shot(target, &branch);
            let summary = pr(480 + i as u64, &branch, "Lane PR", state, checks);
            prs.push(summary.clone());
            self.seed_pr_lookup_for_shot(
                target,
                PrLookup::Linked(Box::new(PrLink {
                    repo: repo(),
                    pr: summary,
                })),
                cx,
            );
        }
        install_fake_gh(GhAuth::Ready, prs, cx);
        PrPoller::seed_for_shot(GITHUB_HOST, GhAvailability::Ready, cx);
        cx.notify();
    }
}

/// How long ago a capture's pull request last changed, as GitHub writes it.
fn updated_ago(minutes: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::minutes(minutes)).to_rfc3339()
}

/// One row of the capture's list: `summary`, opened by `author` against
/// main, from the author's fork when it is cross-repository.
fn list_item(summary: PrSummary, author: &str, comments: u32) -> PrListItem {
    let repo = repo();
    let head_owner = if summary.cross_repo {
        author.to_owned()
    } else {
        repo.owner().to_owned()
    };
    PrListItem {
        repo,
        pr: summary,
        author: Some(author.into()),
        head_owner: Some(head_owner),
        base_branch: "main".into(),
        comments,
    }
}

impl Workspace {
    /// The Pull requests page over two repositories: two pull requests linked
    /// to worktrees, a draft without one, a fork PR with its review worktree,
    /// and a repository with nothing open. Answers the `gh` it installs, and
    /// the row of the active worktree's failing pull request.
    pub(in crate::workspace) fn seed_pr_page_for_shot(
        &mut self,
        cx: &mut Context<Self>,
    ) -> (FakeGh, PrListItem) {
        let project = self.active.project;
        let (fix, review) = (LaneRef { project, lane: 78 }, LaneRef { project, lane: 79 });
        if let Some(record) = self.project_for_mut(project) {
            for (lane, name) in [(fix, "fix-scroll"), (review, "pr-478")] {
                let mut added =
                    crate::lane::Lane::default_for_project(lane.lane, record.root.clone());
                added.name = Some(name.into());
                added.tab_order = record.lanes.len() as u32;
                record.lanes.push(added);
            }
        }
        let mut notes = crate::project::Project::bootstrap_placeholder(
            self.next_project_id,
            self.data_dir.clone(),
        );
        self.next_project_id += 1;
        notes.name = "obsidian-sync".into();
        notes.tab_order = self.projects.len() as u32;
        let notes_lane = LaneRef {
            project: notes.id,
            lane: notes.lanes[0].id,
        };
        self.projects.push(notes);

        let active = self.active;
        self.seed_push_target_for_shot(active, "sidebar-study");
        self.seed_push_target_for_shot(fix, "fix-scroll");
        self.seed_push_to_for_shot(notes_lane, "git@github.com:woo/obsidian-sync.git", "main");
        self.lane_scoped_mut(review).git.tracking = Some(GitTracking {
            branch: Some("pr-478".into()),
            upstream: None,
            ahead: 0,
            behind: 0,
        });

        let with = |mut summary: PrSummary, review, minutes| {
            summary.review = review;
            summary.updated_at = updated_ago(minutes);
            summary
        };
        let sidebar = with(
            pr(
                480,
                "sidebar-study",
                "Adjust sidebar presentation for narrow docks",
                PrState::Open,
                CheckRollup::Failing { failed: Some(2) },
            ),
            Some(ReviewDecision::ChangesRequested),
            120,
        );
        let scroll = with(
            pr(
                483,
                "fix-scroll",
                "Keep chat scroll position across re-parse",
                PrState::Open,
                CheckRollup::Passing,
            ),
            Some(ReviewDecision::Approved),
            26 * 60,
        );
        let draft = with(
            pr(
                486,
                "gh-page",
                "GitHub pull requests page",
                PrState::Draft,
                CheckRollup::Pending,
            ),
            Some(ReviewDecision::ReviewRequired),
            5,
        );
        let mut fork = with(
            pr(
                478,
                "ghostty-bump",
                "Bump ghostty submodule",
                PrState::Open,
                CheckRollup::Passing,
            ),
            Some(ReviewDecision::ReviewRequired),
            3 * 24 * 60,
        );
        fork.cross_repo = true;
        // A fork's pull request nobody has opened a worktree for yet, beside
        // the same-repository draft: the two labels the lane column offers.
        let mut unreviewed = with(
            pr(
                475,
                "font-fallback",
                "Fall back to a CJK font for terminal glyphs",
                PrState::Open,
                CheckRollup::Pending,
            ),
            Some(ReviewDecision::ReviewRequired),
            4 * 24 * 60,
        );
        unreviewed.cross_repo = true;

        for (lane, summary) in [(active, &sidebar), (fix, &scroll)] {
            self.seed_pr_lookup_for_shot(
                lane,
                PrLookup::Linked(Box::new(PrLink {
                    repo: repo(),
                    pr: summary.clone(),
                })),
                cx,
            );
        }
        let gh = FakeGh::new();
        gh.set_auth(GITHUB_HOST, GhAuth::Ready);
        gh.set_prs(&repo(), vec![sidebar.clone(), scroll.clone()]);
        let sidebar_row = list_item(sidebar.clone(), "comelf", 3);
        gh.set_pr_list(
            PrFilter::Mine,
            vec![
                list_item(draft, "comelf", 0),
                sidebar_row.clone(),
                list_item(scroll, "comelf", 0),
                list_item(fork, "contributor", 1),
                list_item(unreviewed, "hanji", 2),
            ],
        );
        GhHost::set_global(Arc::new(gh.clone()), cx);
        PrPoller::seed_for_shot(GITHUB_HOST, GhAvailability::Ready, cx);
        self.show_page(Page::PullRequests, cx);
        cx.notify();
        (gh, sidebar_row)
    }

    /// The detail of the active worktree's pull request on its Checks tab,
    /// as `shot` says.
    pub(in crate::workspace) fn seed_pr_detail_for_shot(
        &mut self,
        shot: PrDetailShot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (gh, row) = self.seed_pr_page_for_shot(cx);
        let test = ActionsJob {
            run: 9120,
            job: 4410,
        };
        let clippy = ActionsJob {
            run: 9120,
            job: 4411,
        };
        let check =
            |name: &str, status, job: Option<ActionsJob>, seconds: Option<u64>| CheckEntry {
                run: CheckRun {
                    kind: CheckKind::Run,
                    workflow: Some("CI".into()),
                    event: Some("pull_request".into()),
                    name: name.into(),
                    status,
                    details_url: None,
                    started_at: None,
                    completed_at: None,
                },
                job,
                duration: seconds.map(std::time::Duration::from_secs),
            };
        let failed = |name: &str, job| FailedCheck {
            workflow: Some("CI".into()),
            event: Some("pull_request".into()),
            name: name.into(),
            details_url: None,
            job: Some(job),
        };
        let mut summary = row.pr.clone();
        summary.failed = vec![
            failed("test (ubuntu-latest)", test),
            failed("clippy", clippy),
        ];
        let merge_ready = shot == PrDetailShot::MergeConfirm;
        let detail = PrDetail {
            summary,
            body: "Narrow docks clipped the worktree names.\n\n- Measure the label once per frame\n- Keep the badge column fixed".into(),
            head_sha: "4f9c2e1".into(),
            base_branch: "main".into(),
            author: Some("comelf".into()),
            mergeability: if merge_ready {
                Mergeability::Ready
            } else {
                Mergeability::Blocked(BlockReason::Requirements)
            },
            methods: vec![MergeMethod::Merge, MergeMethod::Squash, MergeMethod::Rebase],
            default_method: None,
            reviewers: vec![
                ("hanji".into(), ReviewerState::ChangesRequested),
                ("contributor".into(), ReviewerState::Commented),
                ("woo-bot".into(), ReviewerState::Requested),
            ],
            checks: vec![
                check("test (ubuntu-latest)", CheckStatus::Failure, Some(test), Some(252)),
                check("clippy", CheckStatus::Failure, Some(clippy), Some(125)),
                check("build (windows-latest)", CheckStatus::Pending, None, None),
                check("fmt", CheckStatus::Success, None, Some(31)),
                check("build (macos-14)", CheckStatus::Success, None, Some(400)),
                check("docs preview", CheckStatus::Ignored, None, None),
            ],
            open_threads: 3,
        };
        gh.set_pr_detail(&row.repo, row.pr.number, detail.clone());
        gh.set_failed_log(
            test,
            "test (ubuntu-latest)\tRun tests\t2026-11-13T02:11:04.1Z ##[group]Run cargo test --locked -p daruda\n\
             test (ubuntu-latest)\tRun tests\t2026-11-13T02:14:51.2Z test workspace::left_dock::narrow_dock_keeps_names ... FAILED\n\
             test (ubuntu-latest)\tRun tests\t2026-11-13T02:14:51.3Z thread panicked at left_dock/tests.rs:212:5:\n\
             test (ubuntu-latest)\tRun tests\t2026-11-13T02:14:51.3Z assertion `left == right` failed: label clipped\n\
             test (ubuntu-latest)\tRun tests\t2026-11-13T02:14:52.0Z ##[error]Process completed with exit code 101.",
        );
        let item = row.clone();
        self.open_pr_detail(row, window, cx);
        if let Some(id) = self
            .pages
            .pull_requests
            .detail
            .as_ref()
            .filter(|d| d.item == item)
            .map(|d| d.id)
        {
            match shot {
                PrDetailShot::FailedLog => self.toggle_pr_check_log(id, test, cx),
                PrDetailShot::MergeConfirm => {
                    // The dropdown's open state lives inside the widget, out
                    // of reach of a capture, so the shot is the dialog a
                    // menu item raises; it needs the detail already read.
                    if let Some(shown) = self.pages.pull_requests.detail.as_mut() {
                        shown.load = Load::Ready(detail);
                    }
                    self.ask_pr_merge(id, MergeMethod::Squash, window, cx);
                }
            }
        }
    }
}
