//! Opening a worktree from a Pull requests page row: what git is asked, in
//! what order, and what is left when it fails.

use daruda_github::fake::FakeGh;
use daruda_github::{CheckRollup, PrFilter, PrListItem, PrState, PrSummary, RepoId};
use daruda_store::project::{LaneKind, LaneRef};
use gpui::{TestAppContext, VisualTestContext};

use crate::lane::git::{FakeFailure, FakeRepo, GitCall, GitOp, LaneStart, PushTarget};
use crate::surface::strings::error;
use crate::workspace::pages::Page;
use crate::workspace::pages::pull_requests::list::{GroupBody, LaneSlot, PageBody};
use crate::workspace::tests::{GitFixture, fake_lane};

fn item(number: u64, head: &str, cross_repo: bool) -> PrListItem {
    PrListItem {
        repo: RepoId::new("github.com", "o", "r"),
        pr: PrSummary {
            number,
            url: format!("https://github.com/o/r/pull/{number}"),
            title: format!("PR {number}"),
            state: PrState::Open,
            head_branch: head.into(),
            cross_repo,
            checks: CheckRollup::Passing,
            failed: Vec::new(),
            review: None,
            updated_at: "2026-01-01T00:00:00Z".into(),
        },
        author: Some("alice".into()),
        head_owner: None,
        base_branch: "main".into(),
        comments: 0,
    }
}

/// A window whose one lane, on `main`, pushes to o/r through `upstream`,
/// and the `gh` it asks.
fn window(
    cx: &mut TestAppContext,
    repo: impl FnOnce(FakeRepo) -> FakeRepo,
) -> (GitFixture, FakeGh) {
    let gh = crate::test_support::fake_gh(cx);
    let f = GitFixture::open(cx, &daruda_config::Config::default(), |r: FakeRepo| {
        repo(r.push_target(PushTarget {
            remote: "upstream".into(),
            url: "git@github.com:o/r.git".into(),
            branch: "main".into(),
            published: true,
            has_upstream: true,
            review: None,
        }))
    });
    let id = f.ws.read_with(cx, |ws, _| ws.active_ref());
    f.ws.update(cx, |ws, cx| ws.refresh_git_status(id, cx));
    cx.run_until_parked();
    (f, gh)
}

/// Click a row's button, as the page does, without waiting for git.
fn click(f: &GitFixture, item: &PrListItem, cx: &mut TestAppContext) {
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    let item = item.clone();
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| ws.create_lane_from_pr(item, None, window, cx));
    });
}

fn calls(f: &GitFixture) -> Vec<GitCall> {
    f.fake
        .calls()
        .into_iter()
        .filter(|call| {
            matches!(
                call,
                GitCall::FetchRef { .. }
                    | GitCall::MoveBranch { .. }
                    | GitCall::SetPushRemote { .. }
                    | GitCall::SetReviewMark { .. }
                    | GitCall::AddLane { .. }
            )
        })
        .collect()
}

/// What was reported about making a worktree. The fixture's watcher also
/// reports its missing git directory, which is not this feature's to judge.
fn error_titles(f: &GitFixture, cx: &mut TestAppContext) -> Vec<String> {
    let ours = [
        error::lane_create_busy(),
        error::lane_create_failed(),
        error::lane_review_branch_taken("pr-9"),
        error::lane_kept_local_commits("pr-9"),
    ];
    f.ws.read_with(cx, |ws, _| {
        ws.error_history()
            .iter()
            .map(|report| report.title.clone())
            .filter(|title| ours.contains(title))
            .collect()
    })
}

/// Whether the repository's lock is free, leaving it free.
fn lock_free(f: &GitFixture, cx: &mut TestAppContext) -> bool {
    f.ws.update(cx, |ws, _| {
        ws.lock_repo(&f.root)
            .map(|lock| ws.release_repo_lock(lock))
            .is_some()
    })
}

/// A worktree git made for review branch `pr-9`, reading back the mark
/// the checkout wrote, as real git's config would.
fn review_lane(f: &GitFixture, path: &std::path::Path) {
    std::fs::create_dir_all(path).unwrap();
    f.fake
        .add_repo(FakeRepo::at(path).branch("pr-9").push_target(PushTarget {
            remote: "origin".into(),
            url: "git@github.com:o/r.git".into(),
            branch: "pr-9".into(),
            published: false,
            has_upstream: false,
            review: Some(MARK.into()),
        }));
}

const MARK: &str = "github.com/o/r#9";

#[gpui::test]
fn a_pull_request_opens_on_a_branch_brought_to_its_fetched_head(cx: &mut TestAppContext) {
    // (label, pull request, local branch, refspec, start, review mark)
    let cases = [
        (
            "same repository: its own branch, so a push updates it",
            item(8, "fix/scroll", false),
            "fix/scroll",
            "+refs/heads/fix/scroll:refs/remotes/upstream/fix/scroll",
            "refs/remotes/upstream/fix/scroll",
            None,
        ),
        (
            "fork: a review branch marked with its pull request",
            item(9, "feature", true),
            "pr-9",
            "+refs/pull/9/head:refs/daruda/pr/9",
            "refs/daruda/pr/9",
            Some(MARK),
        ),
    ];
    for (label, pr, branch, refspec, start, review) in cases {
        let (f, _gh) = window(cx, |repo| repo);
        let new_path = crate::workspace::lane_ops::lane_checkout_path(&f.root, branch);
        fake_lane(&f.fake, &new_path, branch);
        click(&f, &pr, cx);
        cx.run_until_parked();

        let mut expected = vec![GitCall::FetchRef {
            repo: f.root.clone(),
            remote: "git@github.com:o/r.git".into(),
            refspec: refspec.into(),
        }];
        expected.push(GitCall::MoveBranch {
            repo: f.root.clone(),
            branch: branch.into(),
            head: start.into(),
        });
        // Set once the branch exists, so deleting the branch removes it.
        if !pr.pr.cross_repo {
            expected.push(GitCall::SetPushRemote {
                repo: f.root.clone(),
                branch: branch.into(),
                remote: "upstream".into(),
            });
        }
        expected.extend(review.map(|mark| GitCall::SetReviewMark {
            repo: f.root.clone(),
            branch: branch.into(),
            mark: mark.into(),
        }));
        expected.push(GitCall::AddLane {
            repo: f.root.clone(),
            new_path: new_path.clone(),
            start: LaneStart::existing_branch(branch),
        });
        assert_eq!(calls(&f), expected, "{label}");
        f.ws.read_with(cx, |ws, _| {
            let lane = ws.active_lane().expect("the new lane is active");
            assert_eq!(lane.path, new_path, "{label}");
            assert!(
                matches!(&lane.kind, LaneKind::Git { branch: Some(b), .. } if b == branch),
                "{label}: {:?}",
                lane.kind
            );
            assert_eq!(lane.base_ref.as_deref(), Some("upstream/main"), "{label}");
            assert_eq!(
                lane.description.is_some(),
                pr.pr.cross_repo,
                "{label}: only a review worktree says what it is"
            );
        });
        assert!(lock_free(&f, cx), "{label}: the lock is released");
        assert!(error_titles(&f, cx).is_empty(), "{label}");
    }
}

#[gpui::test]
fn a_branch_with_commits_of_its_own_is_kept_and_the_user_told(cx: &mut TestAppContext) {
    let (f, _gh) = window(cx, |repo| {
        repo.ahead_branches(["pr-9"]).review_mark("pr-9", MARK)
    });
    let new_path = crate::workspace::lane_ops::lane_checkout_path(&f.root, "pr-9");
    review_lane(&f, &new_path);
    click(&f, &item(9, "feature", true), cx);
    cx.run_until_parked();
    assert_eq!(
        calls(&f).last(),
        Some(&GitCall::AddLane {
            repo: f.root.clone(),
            new_path: new_path.clone(),
            start: LaneStart::existing_branch("pr-9"),
        })
    );
    assert!(
        !calls(&f)
            .iter()
            .any(|call| matches!(call, GitCall::MoveBranch { .. })),
        "the commits stay: {:?}",
        calls(&f)
    );
    f.ws.read_with(cx, |ws, _| {
        assert_eq!(ws.active_lane().unwrap().path, new_path)
    });
    assert_eq!(
        error_titles(&f, cx),
        vec![error::lane_kept_local_commits("pr-9")]
    );
}

#[gpui::test]
fn a_users_own_branch_named_like_the_review_branch_is_not_taken(cx: &mut TestAppContext) {
    let (f, _gh) = window(cx, |repo| repo.local_branches(["pr-9"]));
    let main = f.ws.read_with(cx, |ws, _| ws.active_ref());
    let lanes_before = f.ws.read_with(cx, |ws, _| ws.projects.lanes().count());
    click(&f, &item(9, "feature", true), cx);
    cx.run_until_parked();
    assert!(calls(&f).is_empty(), "{:?}", calls(&f));
    assert_eq!(
        error_titles(&f, cx),
        vec![error::lane_review_branch_taken("pr-9")]
    );
    assert!(lock_free(&f, cx));
    f.ws.read_with(cx, |ws, _| {
        assert_eq!(ws.active_ref(), main);
        assert_eq!(ws.projects.lanes().count(), lanes_before);
    });
}

#[gpui::test]
fn a_second_click_goes_to_the_worktree_the_first_made(cx: &mut TestAppContext) {
    let (f, _gh) = window(cx, |repo| repo);
    let main = f.ws.read_with(cx, |ws, _| ws.active_ref());
    let pr = item(9, "feature", true);
    let new_path = crate::workspace::lane_ops::lane_checkout_path(&f.root, "pr-9");
    review_lane(&f, &new_path);
    click(&f, &pr, cx);
    cx.run_until_parked();
    let made = f.ws.read_with(cx, |ws, _| ws.active_ref());
    assert_ne!(made, main);
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.update(|window, cx| f.ws.update(cx, |ws, cx| ws.activate_lane(main, window, cx)));
    click(&f, &pr, cx);
    cx.run_until_parked();
    assert_eq!(f.ws.read_with(cx, |ws, _| ws.active_ref()), made);
    assert_eq!(calls(&f).len(), 4, "git ran once: {:?}", calls(&f));
    assert!(
        error_titles(&f, cx).is_empty(),
        "found by its mark, not refused as someone else's pr-9"
    );
}

#[gpui::test]
fn a_linked_worktree_is_gone_to_without_git(cx: &mut TestAppContext) {
    let gh = crate::test_support::fake_gh(cx);
    let pr = item(7, "feat", false);
    gh.set_prs(&pr.repo, vec![pr.pr.clone()]);
    let f = GitFixture::open(cx, &daruda_config::Config::default(), |repo: FakeRepo| {
        repo.push_target(PushTarget {
            remote: "origin".into(),
            url: "git@github.com:o/r.git".into(),
            branch: "feat".into(),
            published: true,
            has_upstream: false,
            review: None,
        })
    });
    let linked = f.ws.read_with(cx, |ws, _| ws.active_ref());
    f.ws.update(cx, |ws, cx| ws.refresh_git_status(linked, cx));
    cx.run_until_parked();
    let (window, ws) = (f.window, f.ws.downgrade());
    cx.update(|cx| crate::window_registry::WindowRegistry::register(window.into(), ws, cx));
    cx.update(crate::github::PrPoller::tick);
    cx.run_until_parked();
    // Standing on another lane shows the click goes to the linked one.
    let other_dir = f.root.parent().unwrap().join("other");
    std::fs::create_dir_all(&other_dir).unwrap();
    let other = LaneRef {
        project: linked.project,
        lane: 77,
    };
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| {
            let lane = crate::lane::Lane::default_for_project(77, other_dir.clone());
            ws.project_for_mut(linked.project).unwrap().lanes.push(lane);
            ws.activate_lane(other, window, cx);
        })
    });
    click(&f, &pr, cx);
    cx.run_until_parked();
    assert_eq!(f.ws.read_with(cx, |ws, _| ws.active_ref()), linked);
    assert!(calls(&f).is_empty(), "{:?}", calls(&f));
}

#[gpui::test]
fn nothing_is_made_while_the_repository_is_locked(cx: &mut TestAppContext) {
    let (f, _gh) = window(cx, |repo| repo);
    // Held for the rest of the test: the click must find it taken.
    let held = f.ws.update(cx, |ws, _| ws.lock_repo(&f.root));
    assert!(held.is_some());
    click(&f, &item(8, "fix/scroll", false), cx);
    cx.run_until_parked();
    assert!(calls(&f).is_empty(), "{:?}", calls(&f));
    assert_eq!(error_titles(&f, cx), vec![error::lane_create_busy()]);
    assert!(!lock_free(&f, cx), "the holder's lock is not released");
}

#[gpui::test]
fn a_failed_fetch_is_reported_and_releases_the_row_and_the_lock(cx: &mut TestAppContext) {
    let (f, gh) = window(cx, |repo| repo);
    f.fake.fail(
        f.root.clone(),
        GitOp::FetchRef,
        FakeFailure::Exit {
            code: Some(128),
            stderr: "fatal: couldn't find remote ref".into(),
        },
    );
    let gh_list = item(8, "fix/scroll", false);
    let main = f.ws.read_with(cx, |ws, _| ws.active_ref());
    let lanes_before = f.ws.read_with(cx, |ws, _| ws.projects.lanes().count());
    // The page lists the PR, so the in-flight row can be read.
    gh.set_pr_list(PrFilter::Mine, vec![gh_list.clone()]);
    let (window, ws) = (f.window, f.ws.downgrade());
    cx.update(|cx| crate::window_registry::WindowRegistry::register(window.into(), ws, cx));
    f.ws.update(cx, |ws, cx| ws.show_page(Page::PullRequests, cx));
    cx.update(crate::github::PrPoller::tick);
    cx.run_until_parked();
    assert_eq!(slot(&f, cx), Slot::Absent);

    click(&f, &gh_list, cx);
    assert_eq!(slot(&f, cx), Slot::Creating, "inert while git runs");
    cx.run_until_parked();

    assert_eq!(slot(&f, cx), Slot::Absent, "offered again");
    assert!(
        !calls(&f)
            .iter()
            .any(|call| matches!(call, GitCall::AddLane { .. })),
        "nothing is added after a failed fetch: {:?}",
        calls(&f)
    );
    assert_eq!(error_titles(&f, cx), vec![error::lane_create_failed()]);
    assert!(lock_free(&f, cx));
    f.ws.read_with(cx, |ws, _| {
        assert_eq!(ws.active_ref(), main);
        assert_eq!(ws.projects.lanes().count(), lanes_before);
    });
}

#[derive(Debug, PartialEq)]
enum Slot {
    Linked,
    Creating,
    Absent,
}

/// The lane slot of the page's one row.
fn slot(f: &GitFixture, cx: &mut TestAppContext) -> Slot {
    let list = f.ws.read_with(cx, |ws, cx| ws.pr_page_list(cx));
    let PageBody::Groups(groups) = list.body else {
        panic!("expected groups, got {:?}", list.body);
    };
    let GroupBody::Listed { rows, .. } = &groups[0].body else {
        panic!("{:?}", groups[0].body);
    };
    match &rows[0].lane {
        LaneSlot::Linked(_) => Slot::Linked,
        LaneSlot::Creating { .. } => Slot::Creating,
        LaneSlot::Absent { .. } => Slot::Absent,
    }
}

/// The detail's worktree slot, staged as the page draws it.
fn detail_slot(f: &GitFixture, cx: &mut TestAppContext) -> LaneSlot {
    f.ws.update(cx, |ws, cx| {
        ws.stage_pr_page(cx);
        ws.pages.pull_requests.detail_snapshot.clone()
    })
    .expect("the page shows its detail")
    .lane
}

#[gpui::test]
fn the_details_button_makes_the_worktree_its_row_would(cx: &mut TestAppContext) {
    let (f, gh) = window(cx, |repo| repo);
    // Failing, so the in-flight state can be read before it settles.
    f.fake.fail(
        f.root.clone(),
        GitOp::FetchRef,
        FakeFailure::Exit {
            code: Some(128),
            stderr: "fatal: couldn't find remote ref".into(),
        },
    );
    let pr = item(8, "fix/scroll", false);
    gh.set_pr_list(PrFilter::Mine, vec![pr.clone()]);
    let (window, ws) = (f.window, f.ws.downgrade());
    cx.update(|cx| crate::window_registry::WindowRegistry::register(window.into(), ws, cx));
    f.ws.update(cx, |ws, cx| ws.show_page(Page::PullRequests, cx));
    cx.update(crate::github::PrPoller::tick);
    cx.run_until_parked();
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| ws.open_pr_detail(pr.clone(), window, cx));
    });
    cx.run_until_parked();
    assert_eq!(detail_slot(&f, cx), LaneSlot::Absent { review: false });

    let id =
        f.ws.read_with(cx, |ws, _| ws.pr_detail_id())
            .expect("shown");
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| ws.create_pr_detail_lane(id, window, cx));
    });
    assert_eq!(
        detail_slot(&f, cx),
        LaneSlot::Creating { review: false },
        "inert while git runs"
    );
    assert_eq!(slot(&f, cx), Slot::Creating, "and so is its row");
    cx.run_until_parked();

    assert_eq!(
        calls(&f),
        vec![GitCall::FetchRef {
            repo: f.root.clone(),
            remote: "git@github.com:o/r.git".into(),
            refspec: "+refs/heads/fix/scroll:refs/remotes/upstream/fix/scroll".into(),
        }]
    );
    assert_eq!(detail_slot(&f, cx), LaneSlot::Absent { review: false });
    assert_eq!(error_titles(&f, cx), vec![error::lane_create_failed()]);
}

/// Open `f`'s repository a second time, as another project whose lane
/// pushes to the same o/r, and browse every project.
fn second_copy(f: &GitFixture, cx: &mut TestAppContext) {
    f.ws.update(cx, |ws, cx| {
        let push = ws
            .lane_scoped
            .get(&ws.active)
            .unwrap()
            .git
            .push_target
            .clone();
        let project = ws.next_project_id;
        ws.next_project_id += 1;
        let other = crate::project::Project::bootstrap_placeholder(
            project,
            std::env::temp_dir().join("daruda-pr-second-copy"),
        );
        let lane = LaneRef {
            project,
            lane: other.lanes[0].id,
        };
        ws.projects.push(other);
        ws.lane_scoped_mut(lane).git.push_target = push;
        ws.set_task_scope(daruda_store::tasks::TaskScope::AllProjects, cx);
    });
}

#[gpui::test]
fn aggregate_worktree_creation_asks_which_copy_of_a_repo_to_use(cx: &mut TestAppContext) {
    let (f, _gh) = window(cx, |r| r);
    second_copy(&f, cx);
    let active = f.ws.read_with(cx, |ws, _| ws.active);
    click(&f, &item(9, "feature", false), cx);
    assert!(
        cx.has_pending_prompt(),
        "a physical project must be chosen before git runs"
    );
    assert!(calls(&f).is_empty());
    cx.simulate_prompt_answer(&crate::surface::strings::common::btn_cancel());
    cx.run_until_parked();
    assert!(calls(&f).is_empty());
    assert_eq!(f.ws.read_with(cx, |ws, _| ws.active), active);
}

#[gpui::test]
fn choosing_the_copy_makes_the_worktree_there_and_keeps_the_browsed_scope(cx: &mut TestAppContext) {
    use crate::workspace::pages::scope::ProjectScope;
    let (f, _gh) = window(cx, |r| r);
    second_copy(&f, cx);
    let first = f.ws.read_with(cx, |ws, _| {
        let project = ws.project_for(ws.active.project).unwrap();
        crate::surface::strings::pull_requests::project_choice(
            &project.name,
            project.root.display(),
        )
    });
    click(&f, &item(9, "feature", false), cx);
    cx.simulate_prompt_answer(&first);
    cx.run_until_parked();
    assert!(
        calls(&f)
            .iter()
            .any(|call| matches!(call, GitCall::FetchRef { repo, .. } if *repo == f.root)),
        "git runs in the chosen copy: {:?}",
        calls(&f)
    );
    assert_eq!(
        f.ws.read_with(cx, |ws, _| ws.page_scope()),
        ProjectScope::All,
        "choosing where to make a worktree does not narrow the page"
    );
}
