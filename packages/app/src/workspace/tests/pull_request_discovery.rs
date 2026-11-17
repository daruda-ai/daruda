//! PR discovery starts from the project's worktrees, before any pane visit
//! or Git Changes refresh has populated their lane-scoped caches.

use std::collections::BTreeMap;

use daruda_github::fake::{FakeGh, GhOp};
use daruda_github::{CheckRollup, PrFilter, PrListItem, PrState, PrSummary, RepoId};
use daruda_store::project::ProjectUuid;
use gpui::TestAppContext;

use crate::lane::git::{FakeRepo, GitOp, PushTarget};
use crate::workspace::pages::Page;
use crate::workspace::pages::pull_requests::list::{GroupBody, PageBody};
use crate::workspace::pages::scope::ProjectScope;

use super::{GitFixture, build_workspace_with, fake_repo_with};

fn repo(name: &str) -> RepoId {
    RepoId::new("github.com", "o", name)
}

fn pushing_to(git: FakeRepo, name: &str) -> FakeRepo {
    git.push_target(PushTarget {
        remote: "origin".into(),
        url: format!("git@github.com:o/{name}.git"),
        branch: "main".into(),
        published: true,
        has_upstream: false,
        review: None,
    })
}

fn item(name: &str, number: u64) -> PrListItem {
    PrListItem {
        repo: repo(name),
        pr: PrSummary {
            number,
            url: format!("https://github.com/o/{name}/pull/{number}"),
            title: format!("PR {number}"),
            state: PrState::Open,
            head_branch: format!("feat-{number}"),
            cross_repo: false,
            checks: CheckRollup::None,
            failed: Vec::new(),
            review: None,
            updated_at: String::new(),
        },
        author: None,
        head_owner: None,
        base_branch: "main".into(),
        comments: 0,
    }
}

fn cold_window(cx: &mut TestAppContext) -> (GitFixture, FakeGh) {
    let gh = crate::test_support::fake_gh(cx);
    for filter in [PrFilter::Mine, PrFilter::ReviewRequested, PrFilter::AllOpen] {
        gh.set_pr_list(filter, vec![item("r", 7), item("s", 8), item("t", 9)]);
    }
    let f = GitFixture::open(cx, &daruda_config::Config::default(), |git| {
        pushing_to(git, "r")
    });
    let (window, workspace) = (f.window, f.ws.downgrade());
    cx.update(|cx| crate::window_registry::WindowRegistry::register(window.into(), workspace, cx));
    assert_eq!(f.fake.reads(GitOp::PushTarget, &f.root), 0);
    (f, gh)
}

fn listed(f: &GitFixture, cx: &mut TestAppContext) -> BTreeMap<RepoId, Vec<u64>> {
    let page = f.ws.read_with(cx, |ws, cx| ws.pr_page_list(cx));
    let PageBody::Groups(groups) = page.body else {
        panic!("expected repository groups, got {:?}", page.body);
    };
    groups
        .into_iter()
        .map(|group| {
            let GroupBody::Listed { rows, trouble, .. } = group.body else {
                panic!("{}: {:?}", group.label, group.body);
            };
            assert_eq!(trouble, None, "{}", group.label);
            (
                group.repo,
                rows.iter().map(|row| row.item.pr.number).collect(),
            )
        })
        .collect()
}

#[gpui::test]
fn pull_request_page_discovers_repositories_without_a_pane_visit(cx: &mut TestAppContext) {
    let (f, gh) = cold_window(cx);
    let active = f.ws.read_with(cx, |ws, _| ws.active_ref());
    f.ws.update(cx, |ws, cx| {
        ws.show_page(Page::PullRequests, cx);
        assert_eq!(ws.pr_page_list(cx).body, PageBody::Discovering);
    });
    cx.run_until_parked();
    for filter in [PrFilter::Mine, PrFilter::ReviewRequested, PrFilter::AllOpen] {
        f.ws.update(cx, |ws, cx| ws.set_pr_page_filter(filter, cx));
        cx.run_until_parked();
        assert_eq!(listed(&f, cx), BTreeMap::from([(repo("r"), vec![7])]));
    }
    assert_eq!(f.ws.read_with(cx, |ws, _| ws.active_ref()), active);
    assert_eq!(gh.count(GhOp::SearchPrs), 3, "tab answers are reused");
}

#[gpui::test]
fn pull_request_scope_discovers_unvisited_projects(cx: &mut TestAppContext) {
    let (f, _) = cold_window(cx);
    let second_dir = tempfile::tempdir().unwrap();
    let third_dir = tempfile::tempdir().unwrap();
    let mut uuids = Vec::new();
    for (dir, name) in [(&second_dir, "s"), (&third_dir, "t")] {
        let root = fake_repo_with(&f.fake, dir.path(), |git| pushing_to(git, name));
        let uuid = ProjectUuid::new();
        f.ws.update(cx, |ws, _| {
            let id = ws.next_project_id;
            ws.next_project_id += 1;
            ws.projects.push(crate::project::Project::new_with_uuid(
                id, uuid, root, &f.fake,
            ));
        });
        uuids.push(uuid);
    }
    let active = f.ws.read_with(cx, |ws, _| ws.active_ref());
    f.ws.update(cx, |ws, cx| ws.show_page(Page::PullRequests, cx));
    cx.run_until_parked();
    assert_eq!(listed(&f, cx), BTreeMap::from([(repo("r"), vec![7])]));
    // Browsing a project must discover it without activating its worktree.
    f.ws.update(cx, |ws, cx| {
        ws.set_page_scope(ProjectScope::Project(uuids[0]), cx);
    });
    cx.run_until_parked();
    assert_eq!(listed(&f, cx), BTreeMap::from([(repo("s"), vec![8])]));
    f.ws.update(cx, |ws, cx| ws.set_page_scope(ProjectScope::All, cx));
    cx.run_until_parked();
    assert_eq!(
        listed(&f, cx),
        BTreeMap::from([
            (repo("r"), vec![7]),
            (repo("s"), vec![8]),
            (repo("t"), vec![9])
        ])
    );
    assert_eq!(f.ws.read_with(cx, |ws, _| ws.active_ref()), active);
}

#[gpui::test]
fn pull_request_refresh_retries_repository_discovery(cx: &mut TestAppContext) {
    let (f, gh) = cold_window(cx);
    f.fake.add_repo(FakeRepo::at(&f.root).branch("main"));
    f.ws.update(cx, |ws, cx| ws.show_page(Page::PullRequests, cx));
    cx.run_until_parked();
    assert_eq!(gh.count(GhOp::SearchPrs), 0);
    assert_eq!(
        f.ws.read_with(cx, |ws, cx| ws.pr_page_list(cx).body),
        PageBody::NoRepos
    );
    // A remote added since opening the page must be found by Refresh.
    f.fake
        .add_repo(pushing_to(FakeRepo::at(&f.root).branch("main"), "r"));
    f.ws.update(cx, |ws, cx| ws.refresh_pr_list(cx));
    cx.run_until_parked();
    assert_eq!(listed(&f, cx), BTreeMap::from([(repo("r"), vec![7])]));
    assert_eq!(f.fake.reads(GitOp::PushTarget, &f.root), 2);
}

#[gpui::test]
fn enabling_github_discovers_unvisited_worktrees(cx: &mut TestAppContext) {
    let (f, _) = cold_window(cx);
    let set_enabled = |cx: &mut TestAppContext, enabled| {
        cx.update(|cx| {
            let mut config = daruda_config::Config::default();
            config.github.enabled = enabled;
            cx.global_mut::<crate::settings_store::SettingsStore>()
                .set_user_for_testing(config);
        });
    };
    set_enabled(cx, false);
    f.ws.update(cx, |ws, cx| ws.show_page(Page::PullRequests, cx));
    cx.run_until_parked();
    assert_eq!(f.fake.reads(GitOp::PushTarget, &f.root), 0);
    set_enabled(cx, true);
    cx.update(crate::github::PrPoller::tick);
    cx.run_until_parked();
    assert_eq!(listed(&f, cx), BTreeMap::from([(repo("r"), vec![7])]));
}

#[gpui::test]
fn pull_request_page_follows_late_worktree_discovery(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let fake = crate::test_support::fake_git(cx);
    let gh = crate::test_support::fake_gh(cx);
    gh.set_pr_list(PrFilter::Mine, vec![item("r", 7)]);
    let root = fake_repo_with(&fake, temp.path(), |git| pushing_to(git, "r"));
    let (window, workspace) = build_workspace_with(
        cx,
        &daruda_config::Config::default(),
        Some(daruda_store::project::Project::from_path(&root)),
    );
    cx.update(|cx| {
        crate::window_registry::WindowRegistry::register(window.into(), workspace.downgrade(), cx);
    });
    workspace.update(cx, |ws, cx| ws.show_page(Page::PullRequests, cx));
    let active = workspace.read_with(cx, |ws, _| ws.active_ref());
    cx.run_until_parked();
    assert_eq!(
        fake.reads(GitOp::PushTarget, &root),
        0,
        "still a placeholder"
    );
    workspace.update(cx, |ws, cx| {
        ws.reconcile_bootstrapped_lanes(cx);
        assert_eq!(ws.pr_page_list(cx).body, PageBody::Discovering);
    });
    cx.run_until_parked();
    workspace.read_with(cx, |ws, cx| {
        assert_eq!(ws.github_repos(), [repo("r")].into());
        let PageBody::Groups(groups) = ws.pr_page_list(cx).body else {
            panic!("expected the discovered repository");
        };
        assert!(matches!(&groups[0].body, GroupBody::Listed { rows, .. } if rows.len() == 1));
        assert_eq!(ws.active_ref(), active);
    });
}

#[gpui::test]
fn a_non_git_project_finishes_discovery_without_requesting_github(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let fake = crate::test_support::fake_git(cx);
    let gh = crate::test_support::fake_gh(cx);
    let (_, workspace) = build_workspace_with(
        cx,
        &daruda_config::Config::default(),
        Some(daruda_store::project::Project::from_path(temp.path())),
    );
    workspace.update(cx, |ws, cx| {
        ws.show_page(Page::PullRequests, cx);
        ws.reconcile_bootstrapped_lanes(cx);
        ws.reconcile_bootstrapped_lanes(cx);
        assert_eq!(ws.pr_page_list(cx).body, PageBody::Discovering);
    });
    cx.run_until_parked();
    workspace.read_with(cx, |ws, cx| {
        assert_eq!(ws.pr_page_list(cx).body, PageBody::NoRepos);
    });
    assert_eq!(
        fake.reads(GitOp::Probe, temp.path()),
        1,
        "coalesced discovery"
    );
    assert_eq!(gh.count(GhOp::SearchPrs), 0);
}

#[gpui::test]
fn late_pull_request_projects_do_not_reread_existing_repositories(cx: &mut TestAppContext) {
    let (f, _) = cold_window(cx);
    f.ws.update(cx, |ws, cx| {
        ws.set_page_scope(ProjectScope::All, cx);
        ws.show_page(Page::PullRequests, cx);
    });
    cx.run_until_parked();
    let active = f.ws.read_with(cx, |ws, _| ws.active_ref());
    let temp = tempfile::tempdir().unwrap();
    let mut roots = vec![f.root.clone()];
    for (name, in_scope) in [("s", true), ("t", true), ("outside", false)] {
        let root = fake_repo_with(&f.fake, &temp.path().join(name), |git| {
            pushing_to(git, name)
        });
        roots.push(root.clone());
        f.ws.update(cx, |ws, cx| {
            if !in_scope {
                let project = ws.active_project().unwrap().uuid;
                ws.set_page_scope(ProjectScope::Project(project), cx);
            }
        });
        cx.run_until_parked();
        let reads_before: Vec<_> = roots
            .iter()
            .map(|root| f.fake.reads(GitOp::PushTarget, root))
            .collect();
        f.ws.update(cx, |ws, cx| {
            let id = ws.next_project_id;
            ws.next_project_id += 1;
            ws.projects
                .push(crate::project::Project::bootstrap_placeholder(id, root));
            ws.reconcile_bootstrapped_lanes(cx);
        });
        cx.run_until_parked();
        for (index, root) in roots.iter().enumerate() {
            // In scope or not: a worktree outside the browsed project can
            // still be the one working on a listed pull request.
            let expected = reads_before[index] + usize::from(index == roots.len() - 1);
            assert_eq!(
                f.fake.reads(GitOp::PushTarget, root),
                expected,
                "{name} (in scope: {in_scope}): only the discovered project is read: {}",
                root.display()
            );
        }
    }
    assert_eq!(f.ws.read_with(cx, |ws, _| ws.active_ref()), active);
    assert_eq!(listed(&f, cx), BTreeMap::from([(repo("r"), vec![7])]));
}

/// A worktree in another project, never opened, is still found as the one
/// working on its pull request while the page browses a different project:
/// which lanes are read must match which lanes are matched.
#[gpui::test]
fn an_unvisited_worktree_outside_the_scope_is_found_for_its_pull_request(cx: &mut TestAppContext) {
    use crate::workspace::pages::pull_requests::list::LaneSlot;
    let (f, gh) = cold_window(cx);
    gh.set_prs(&repo("r"), vec![item("r", 7).pr]);
    let other_dir = tempfile::tempdir().unwrap();
    let root = fake_repo_with(&f.fake, other_dir.path(), |git| {
        git.branch("feat-7").push_target(PushTarget {
            remote: "origin".into(),
            url: "git@github.com:o/r.git".into(),
            branch: "feat-7".into(),
            published: true,
            has_upstream: false,
            review: None,
        })
    });
    let other = f.ws.update(cx, |ws, _| {
        let id = ws.next_project_id;
        ws.next_project_id += 1;
        ws.projects.push(crate::project::Project::new_with_uuid(
            id,
            ProjectUuid::new(),
            root,
            &f.fake,
        ));
        id
    });
    let browsed =
        f.ws.read_with(cx, |ws, _| ws.active_project().unwrap().uuid);
    f.ws.update(cx, |ws, cx| {
        ws.set_page_scope(ProjectScope::Project(browsed), cx);
        ws.show_page(Page::PullRequests, cx);
    });
    cx.run_until_parked();
    cx.update(crate::github::PrPoller::tick);
    cx.run_until_parked();
    let page = f.ws.read_with(cx, |ws, cx| ws.pr_page_list(cx));
    let PageBody::Groups(groups) = page.body else {
        panic!("expected repository groups, got {:?}", page.body);
    };
    let GroupBody::Listed { rows, .. } = &groups[0].body else {
        panic!("{:?}", groups[0].body);
    };
    let row = rows.iter().find(|row| row.item.pr.number == 7).unwrap();
    assert!(
        matches!(&row.lane, LaneSlot::Linked(lane) if lane.lane.project == other),
        "#7 goes to the other project's worktree, not a new one: {:?}",
        row.lane
    );
}
