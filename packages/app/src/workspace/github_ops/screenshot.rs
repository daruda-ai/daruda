//! Pull-request fixtures for captures. Every capture starts on an empty
//! `FakeGh` (`startup::inspection::before_globals`); these scenarios replace
//! it with one that answers like the seeds, so the running poller does not
//! overwrite what was seeded.

use std::sync::Arc;

use daruda_github::availability::{GhAuth, GhAvailability};
use daruda_github::fake::FakeGh;
use daruda_github::{
    ActionsJob, BlockReason, CheckEntry, CheckKind, CheckRollup, CheckRun, CheckStatus,
    FailedCheck, GITHUB_HOST, MergeMethod, Mergeability, PrAudience, PrDetail, PrListItem, PrState,
    PrSummary, RepoId, ReviewDecision, ReviewerState,
};
use daruda_store::project::LaneRef;
use gpui::{Context, Window};

use crate::gh_host::GhHost;
use crate::github::{PrLink, PrLookup, PrPoller};
use crate::lane::git::{GitTracking, PushTarget};
use crate::workspace::Workspace;
use crate::workspace::pages::Page;

/// An Enterprise host the captures' `gh` is not logged in to.
const CORP_HOST: &str = "ghe.corp";

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
    /// The first failing check's log open, with only part of the check list read.
    FailedLog,
    /// A mergeable pull request, its squash-merge confirmation up.
    MergeConfirm,
    /// The Files tab: an edit, a rename, an added file and a binary one,
    /// the edit's diff shown.
    Files,
    /// The Files tab on the binary file: nothing to draw, its change on
    /// GitHub offered.
    FilesNoLines,
    /// The Files tab deep in a long diff: a step of rows around the lines
    /// gone to, those before them offered.
    FilesEarlier,
    /// Opened from a link to a conversation comment: that comment marked.
    CommentLink,
    /// The Threads tab: an open thread expanded with a reply and a reply
    /// box open, an outdated one folded.
    Threads,
    /// The Threads tab with a reply that failed and could not be confirmed:
    /// Post held, "It did not post" offered.
    ReplyUnconfirmed,
    /// In the merge queue without auto-merge: merge held, Remove from
    /// merge queue offered.
    Queued,
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
            review: None,
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
    /// The Pull requests page over three repositories: two pull requests
    /// linked to worktrees, a draft without one, a fork PR with its review
    /// worktree, a repository with nothing open, and one on an Enterprise
    /// host `gh` is not logged in to. Answers the `gh` it installs, and the
    /// row of the active worktree's failing pull request.
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
        let mut corp = crate::project::Project::bootstrap_placeholder(
            self.next_project_id,
            self.data_dir.clone(),
        );
        self.next_project_id += 1;
        corp.name = "internal-tool".into();
        corp.tab_order = self.projects.len() as u32;
        let corp_lane = LaneRef {
            project: corp.id,
            lane: corp.lanes[0].id,
        };
        self.projects.push(corp);

        let active = self.active;
        self.seed_push_target_for_shot(active, "sidebar-study");
        self.seed_push_target_for_shot(fix, "fix-scroll");
        self.seed_push_to_for_shot(notes_lane, "git@github.com:woo/obsidian-sync.git", "main");
        self.seed_push_to_for_shot(corp_lane, "git@ghe.corp:corp/internal-tool.git", "main");
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
        gh.set_auth(CORP_HOST, GhAuth::LoggedOut);
        gh.set_prs(&repo(), vec![sidebar.clone(), scroll.clone()]);
        let sidebar_row = list_item(sidebar.clone(), "comelf", 3);
        let rows = vec![
            list_item(draft, "comelf", 0),
            sidebar_row.clone(),
            list_item(scroll, "comelf", 0),
            list_item(fork, "contributor", 1),
            list_item(unreviewed, "hanji", 2),
        ];
        gh.set_pr_list(
            PrAudience::ReviewRequested,
            vec![rows[1].clone(), rows[3].clone()],
        );
        gh.set_pr_list(PrAudience::All, rows.clone());
        gh.set_pr_list(PrAudience::Mine, rows);
        GhHost::set_global(Arc::new(gh.clone()), cx);
        PrPoller::seed_for_shot(GITHUB_HOST, GhAvailability::Ready, cx);
        PrPoller::seed_account_for_shot(
            GITHUB_HOST,
            daruda_github::availability::GhAccount {
                login: "comelf".into(),
                source: daruda_github::availability::CredentialSource::Keyring,
            },
            cx,
        );
        PrPoller::seed_for_shot(CORP_HOST, GhAvailability::LoggedOut, cx);
        self.set_page_scope(crate::workspace::pages::scope::ProjectScope::All, cx);
        self.show_page(Page::PullRequests, cx);
        cx.notify();
        (gh, sidebar_row)
    }

    /// github.com waits out a spent rate limit GitHub said resets in 40
    /// minutes; its rows stay, dimmed under the note.
    pub(in crate::workspace) fn seed_pr_rate_limit_for_shot(&mut self, cx: &mut Context<Self>) {
        use daruda_github::availability::{BackoffReason, Resume};
        let until = cx.background_executor().now() + std::time::Duration::from_secs(40 * 60);
        PrPoller::seed_for_shot(
            GITHUB_HOST,
            GhAvailability::Backoff {
                until,
                reason: BackoffReason::RateLimited,
                resume: Resume::Reported,
            },
            cx,
        );
        cx.notify();
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
        let merge_ready = matches!(shot, PrDetailShot::MergeConfirm | PrDetailShot::Queued);
        if !merge_ready {
            summary.checks = CheckRollup::Failing { failed: None };
        }
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
            checks_truncated: !merge_ready,
            open_threads: 3,
            open_threads_more: false,
            viewer_can: daruda_github::ViewerCan::default()
            .with(daruda_github::Permission::Close, true)
            .with(daruda_github::Permission::Reopen, true),
            auto_merge: None,
            queue: (shot == PrDetailShot::Queued).then_some(daruda_github::QueueEntry {
                position: Some(2),
                state: daruda_github::QueueState::AwaitingChecks,
            }),
            node_id: format!("PR_{}", row.pr.number),
            changed_files: None,
        };
        gh.set_pr_detail(&row.repo, row.pr.number, detail.clone());
        let file =
            |path: &str, change, additions, deletions, patch: Option<&str>| daruda_github::PrFile {
                path: path.into(),
                change,
                additions,
                deletions,
                patch: patch.map(str::to_owned),
                blob: Some(format!("blob:{path}")),
            };
        // Long enough that going to its last lines leaves rows undrawn above.
        let long_path = "packages/app/src/workspace/left_dock/rail/metrics.rs";
        let long_lines = 2_400_u32;
        let long = (shot == PrDetailShot::FilesEarlier).then(|| {
            let mut patch = format!("@@ -1,{long_lines} +1,{long_lines} @@\n");
            for line in 1..long_lines {
                patch.push_str(&format!(" const ROW_{line}: f32 = {line}.0;\n"));
            }
            patch.push_str("-const LAST: f32 = 0.0;\n+const LAST: f32 = 1.0;");
            file(
                long_path,
                daruda_github::FileChange::Modified,
                1,
                1,
                Some(&patch),
            )
        });
        gh.set_pr_files(
            &row.repo,
            row.pr.number,
            daruda_github::PrFiles {
                files: long.into_iter().chain([
                    file(
                        "packages/app/src/workspace/left_dock/rail/lanes.rs",
                        daruda_github::FileChange::Modified,
                        4,
                        2,
                        Some(
                            "@@ -61,9 +61,11 @@ fn lane_trigger(\n     let theme = theme::current(cx);\n-    let active = snap.active == target;\n+    let active = snap.active == target && snap.workspace_page.is_none();\n+    // Measured once per frame: the label is clipped otherwise.\n     div()\n-        .w(px(DOCK_RAIL_W))\n+        .w(px(snap.metrics.target(DOCK_RAIL_W)))\n+        .min_w_0()\n         .h(px(snap.metrics.row(theme::DOCK_RAIL_PANE_ROW_H)))",
                        ),
                    ),
                    file(
                        "packages/app/src/workspace/left_dock/rail/badge.rs",
                        daruda_github::FileChange::Renamed {
                            from: "packages/app/src/workspace/left_dock/badge.rs".into(),
                        },
                        0,
                        0,
                        Some(""),
                    ),
                    file(
                        "packages/app/src/workspace/left_dock/rail/tests.rs",
                        daruda_github::FileChange::Added,
                        38,
                        0,
                        Some("@@ -0,0 +1,2 @@\n+use super::*;\n+"),
                    ),
                    file(
                        "assets/icons/ui/rail-collapse.png",
                        daruda_github::FileChange::Added,
                        0,
                        0,
                        None,
                    ),
                ])
                .collect(),
            },
        );
        gh.set_conversation(
            &row.repo,
            row.pr.number,
            daruda_github::Conversation {
                comments: vec![
                    daruda_github::ReviewComment {
                        id: "shot-conversation-1".into(),
                        url: Some(format!("{}#issuecomment-1", row.pr.url)),
                        author: Some("hanji".into()),
                        body:
                            "Could this wait for the rail metrics change? It touches the same rows."
                                .into(),
                    },
                    daruda_github::ReviewComment {
                        id: "shot-conversation-2".into(),
                        author: Some("comelf".into()),
                        body: "Rebased onto it: the diff is smaller now.".into(),
                        ..Default::default()
                    },
                ],
                truncated: false,
            },
        );
        gh.set_review_threads(
            &row.repo,
            row.pr.number,
            daruda_github::ReviewThreads {
                threads: vec![
                    daruda_github::ReviewThread {
                        id: "shot-thread-1".into(),
                        path: "packages/app/src/workspace/left_dock/rail/lanes.rs".into(),
                        lines: Some((62, 63)),
                        comments: vec![
                            daruda_github::ReviewComment {
                                id: "shot-comment-1".into(),
                                url: Some(format!("{}#discussion_r1", row.pr.url)),
                                author: Some("hanji".into()),
                                body: "This hides the label whenever a page is open. \
                                       Is that intended?\n\n```rust\nsnap.workspace_page.is_none()\n```"
                                    .into(),
                            },
                            daruda_github::ReviewComment {
                                id: "shot-comment-2".into(),
                                author: Some("comelf".into()),
                                body: "Yes: the rail shows **pages** then, not lanes.".into(),
                                ..Default::default()
                            },
                        ],
                        ..Default::default()
                    },
                    daruda_github::ReviewThread {
                        id: "shot-thread-2".into(),
                        path: "packages/app/src/workspace/left_dock/badge.rs".into(),
                        lines: Some((14, 14)),
                        outdated: true,
                        comments: vec![daruda_github::ReviewComment {
                            id: "shot-comment-3".into(),
                            author: Some("woo-bot".into()),
                            body: "Consider a named metric here.".into(),
                            ..Default::default()
                        }],
                        ..Default::default()
                    },
                ],
                truncated: false,
            },
        );
        gh.set_failed_log(
            test,
            "test (ubuntu-latest)\tRun tests\t2026-11-13T02:11:04.1Z ##[group]Run cargo test --locked -p daruda\n\
             test (ubuntu-latest)\tRun tests\t2026-11-13T02:14:51.2Z test workspace::left_dock::narrow_dock_keeps_names ... FAILED\n\
             test (ubuntu-latest)\tRun tests\t2026-11-13T02:14:51.3Z thread panicked at left_dock/tests.rs:212:5:\n\
             test (ubuntu-latest)\tRun tests\t2026-11-13T02:14:51.3Z assertion `left == right` failed: label clipped\n\
             test (ubuntu-latest)\tRun tests\t2026-11-13T02:14:52.0Z ##[error]Process completed with exit code 101.",
        );
        let item = row.clone();
        use crate::workspace::pages::pull_requests::detail;
        let opening = match shot {
            PrDetailShot::CommentLink => detail::PrOpening {
                at: Some(detail::OpeningTarget::Comment(
                    detail::anchors::CommentLink {
                        kind: detail::anchors::CommentKind::Conversation(1),
                        url: format!("{}#issuecomment-1", row.pr.url),
                    },
                )),
                read: None,
            },
            _ => detail::PrOpening::default(),
        };
        self.open_pr_detail_as(row, opening, window, cx);
        if let Some(id) = self
            .pages
            .pull_requests
            .detail()
            .filter(|d| *d.item() == item)
            .map(|d| d.id())
        {
            match shot {
                PrDetailShot::FailedLog => self.toggle_pr_check_log(id, test, cx),
                PrDetailShot::Files => {
                    // One file viewed, one changed since it was.
                    let key = (item.repo.clone(), item.pr.number);
                    let progress = &mut self.pages.pull_requests.progress;
                    let tests = "packages/app/src/workspace/left_dock/rail/tests.rs";
                    progress.toggle_viewed(&key, tests, Some(&format!("blob:{tests}")));
                    let badge = "packages/app/src/workspace/left_dock/rail/badge.rs";
                    progress.toggle_viewed(&key, badge, Some("blob:earlier"));
                    self.select_pr_detail_tab(
                        id,
                        crate::workspace::pages::pull_requests::detail::PrTab::Files,
                        cx,
                    );
                    self.toggle_pr_thread(id, "shot-thread-1", cx);
                }
                PrDetailShot::FilesNoLines | PrDetailShot::FilesEarlier => {
                    let (path, line) = match shot {
                        PrDetailShot::FilesEarlier => (long_path, long_lines),
                        _ => ("assets/icons/ui/rail-collapse.png", 1),
                    };
                    // Gone to as a thread's lines are: the file is shown
                    // once its page lands, a file without lines as it is.
                    self.show_pr_thread_in_diff(
                        id,
                        detail::view::ThreadTarget {
                            path: path.into(),
                            side: detail::diff_layout::Side::New,
                            start: line,
                            end: line,
                            revision: "4f9c2e1".into(),
                        },
                        cx,
                    );
                }
                PrDetailShot::CommentLink => {}
                PrDetailShot::Threads => {
                    self.select_pr_detail_tab(
                        id,
                        crate::workspace::pages::pull_requests::detail::PrTab::Threads,
                        cx,
                    );
                    self.toggle_pr_thread(id, "shot-thread-1", cx);
                    self.open_pr_reply(id, "shot-thread-1", window, cx);
                }
                PrDetailShot::ReplyUnconfirmed => {
                    use daruda_github::fake::{GhFailure, GhOp};
                    // Posted before the tab's threads land, the reply has no
                    // last comment to be looked for after, so every read
                    // leaves it unconfirmed, whatever order they answer in.
                    gh.fail(GhOp::ReplyToThread, GhFailure::Timeout);
                    self.select_pr_detail_tab(
                        id,
                        crate::workspace::pages::pull_requests::detail::PrTab::Threads,
                        cx,
                    );
                    self.toggle_pr_thread(id, "shot-thread-1", cx);
                    self.open_pr_reply(id, "shot-thread-1", window, cx);
                    let reply = self.pages.pull_requests.detail().and_then(|d| {
                        d.thread_work()
                            .get("shot-thread-1")
                            .and_then(|work| work.reply.clone())
                    });
                    if let Some(reply) = reply {
                        reply.update(cx, |reply, cx| {
                            reply.set_value("Renamed it in the next push.", window, cx)
                        });
                    }
                    self.post_pr_reply(id, "shot-thread-1", cx);
                }
                PrDetailShot::MergeConfirm => {
                    // The dropdown's open state lives inside the widget, out
                    // of reach of a capture, so the shot is the dialog a
                    // menu item raises; it needs the detail already read.
                    if let Some(shown) = self.pages.pull_requests.detail_mut() {
                        shown.show_read(detail);
                    }
                    self.ask_pr_merge(id, MergeMethod::Squash, window, cx);
                }
                PrDetailShot::Queued => {}
            }
        }
    }
}
