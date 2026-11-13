//! Opening a worktree from a Pull requests page row: what git is asked, in
//! what order, and what is left when it fails.

use daruda_github::fake::FakeGh;
use daruda_github::{CheckRollup, PrFilter, PrListItem, PrState, PrSummary, RepoId};
use daruda_store::project::{LaneKind, LaneRef};
use gpui::{TestAppContext, VisualTestContext};

use crate::lane::git::{FakeFailure, FakeRepo, GitCall, GitOp, PushTarget};
use crate::surface::strings::error;
use crate::workspace::pages::Page;
use crate::workspace::pages::pull_requests::list::{GroupBody, PageBody, RowLaneSlot};
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
        f.ws.update(cx, |ws, cx| ws.create_lane_from_pr(item, window, cx));
    });
}

fn calls(f: &GitFixture) -> Vec<GitCall> {
    f.fake
        .calls()
        .into_iter()
        .filter(|call| matches!(call, GitCall::FetchRef { .. } | GitCall::AddLane { .. }))
        .collect()
}

/// What was reported about making a worktree. The fixture's watcher also
/// reports its missing git directory, which is not this feature's to judge.
fn error_titles(f: &GitFixture, cx: &mut TestAppContext) -> Vec<String> {
    let ours = [error::lane_create_busy(), error::lane_create_failed()];
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
        let free = ws.acquire_repo_lock(&f.root);
        if free {
            ws.release_repo_lock(&f.root);
        }
        free
    })
}

#[gpui::test]
fn a_pull_request_opens_on_a_branch_started_from_its_fetched_head(cx: &mut TestAppContext) {
    // (label, pull request, local branch, refspec, branch, start)
    let cases = [
        (
            "same repository: its own branch, so a push updates it",
            item(8, "fix/scroll", false),
            "fix/scroll",
            "+refs/heads/fix/scroll:refs/remotes/upstream/fix/scroll",
            "refs/remotes/upstream/fix/scroll",
        ),
        (
            "fork: a review branch",
            item(9, "feature", true),
            "pr-9",
            "+refs/pull/9/head:refs/daruda/pr/9",
            "refs/daruda/pr/9",
        ),
    ];
    for (label, pr, branch, refspec, start) in cases {
        let (f, _gh) = window(cx, |repo| repo);
        let new_path = crate::workspace::lane_ops::lane_checkout_path(&f.root, branch);
        fake_lane(&f.fake, &new_path, branch);
        click(&f, &pr, cx);
        cx.run_until_parked();

        assert_eq!(
            calls(&f),
            vec![
                GitCall::FetchRef {
                    repo: f.root.clone(),
                    remote: "upstream".into(),
                    refspec: refspec.into(),
                },
                GitCall::AddLane {
                    repo: f.root.clone(),
                    new_path: new_path.clone(),
                    new_branch: Some(branch.into()),
                    base: Some(start.into()),
                },
            ],
            "{label}"
        );
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
fn an_existing_local_branch_is_checked_out_rather_than_created(cx: &mut TestAppContext) {
    let (f, _gh) = window(cx, |repo| repo.local_branches(["fix/scroll"]));
    let new_path = crate::workspace::lane_ops::lane_checkout_path(&f.root, "fix/scroll");
    fake_lane(&f.fake, &new_path, "fix/scroll");
    click(&f, &item(8, "fix/scroll", false), cx);
    cx.run_until_parked();
    assert_eq!(
        calls(&f).last(),
        Some(&GitCall::AddLane {
            repo: f.root.clone(),
            new_path,
            new_branch: None,
            base: Some("fix/scroll".into()),
        })
    );
    assert_eq!(f.fake.reads(GitOp::HasLocalBranch, &f.root), 1);
}

#[gpui::test]
fn a_second_click_goes_to_the_worktree_the_first_made(cx: &mut TestAppContext) {
    let (f, _gh) = window(cx, |repo| repo);
    let main = f.ws.read_with(cx, |ws, _| ws.active_ref());
    let pr = item(9, "feature", true);
    let new_path = crate::workspace::lane_ops::lane_checkout_path(&f.root, "pr-9");
    fake_lane(&f.fake, &new_path, "pr-9");
    click(&f, &pr, cx);
    cx.run_until_parked();
    let made = f.ws.read_with(cx, |ws, _| ws.active_ref());
    assert_ne!(made, main);

    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.update(|window, cx| f.ws.update(cx, |ws, cx| ws.activate_lane(main, window, cx)));
    click(&f, &pr, cx);
    cx.run_until_parked();
    assert_eq!(f.ws.read_with(cx, |ws, _| ws.active_ref()), made);
    assert_eq!(calls(&f).len(), 2, "git ran once: {:?}", calls(&f));
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
    assert!(f.ws.update(cx, |ws, _| ws.acquire_repo_lock(&f.root)));
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
        RowLaneSlot::Linked(_) => Slot::Linked,
        RowLaneSlot::Creating => Slot::Creating,
        RowLaneSlot::Absent(_) => Slot::Absent,
    }
}
