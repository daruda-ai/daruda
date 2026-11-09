//! Remote-facing git ops in the Git Changes header — the ones whose only
//! visible product is the header's `↑N ↓M` tracking indicator. Driven against
//! a scripted git, with real git dirs on disk so the real watchers attach;
//! what real commits and fetches write is `daruda_project`'s
//! `tests/it/git_remote.rs`.

use super::*;
use crate::lane::git::{
    FakeGit, FakeRepo, GitCall, GitDirs, GitFileEntry, GitOp, GitTracking, GitWorktreeInfo,
    GitWorktreeStatus,
};

const WATCH_REFRESH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

fn behind(n: u32) -> GitTracking {
    GitTracking {
        branch: Some("main".into()),
        upstream: Some("origin/main".into()),
        ahead: 0,
        behind: n,
    }
}

fn lane_info(path: &std::path::Path, branch: &str) -> GitWorktreeInfo {
    GitWorktreeInfo {
        path: path.to_path_buf(),
        branch: Some(branch.into()),
        head: None,
        bare: false,
    }
}

/// A main worktree at `<temp>/work` and a linked one at `<temp>/side-wt`,
/// with git's own dir layout (`work/.git` shared, `work/.git/worktrees/side`
/// the linked lane's) as plain directories.
struct TwoLanes {
    fake: FakeGit,
    linked: std::path::PathBuf,
    common: std::path::PathBuf,
    side: std::path::PathBuf,
    root: std::path::PathBuf,
    _temp: tempfile::TempDir,
}

/// `repos` scripts the two repositories from their roots; the layout and the
/// lane list are added here.
fn two_lanes_with(
    cx: &mut TestAppContext,
    repos: impl FnOnce(&std::path::Path, &std::path::Path) -> (FakeRepo, FakeRepo),
) -> TwoLanes {
    let temp = tempfile::tempdir().unwrap();
    let base = daruda_core::path::canonicalize(temp.path()).unwrap();
    let root = base.join("work");
    let linked = base.join("side-wt");
    let common = root.join(".git");
    let side = common.join("worktrees").join("side");
    for dir in [&linked, &side] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let (main, linked_repo) = repos(&root, &linked);
    let fake = crate::test_support::fake_git(cx);
    fake.add_repo(
        main.worktrees(vec![lane_info(&root, "main"), lane_info(&linked, "side")])
            .dirs(GitDirs {
                git_dir: common.clone(),
                common_dir: common.clone(),
            }),
    );
    fake.add_repo(linked_repo.dirs(GitDirs {
        git_dir: side.clone(),
        common_dir: common.clone(),
    }));
    TwoLanes {
        fake,
        linked,
        common,
        side,
        root,
        _temp: temp,
    }
}

fn lane_refs(ws: &Workspace) -> Vec<daruda_store::project::LaneRef> {
    let project = &ws.projects[0];
    project
        .lanes
        .iter()
        .map(|lane| daruda_store::project::LaneRef {
            project: project.id,
            lane: lane.id,
        })
        .collect()
}

fn tracking_behind(ws: &Workspace, target: &daruda_store::project::LaneRef) -> u32 {
    ws.lane_scoped[target]
        .git
        .tracking
        .as_ref()
        .expect("a git lane has tracking info")
        .behind
}

/// Drive gpui until `predicate` observes a refresh from the real notify
/// watcher. The debounce thread uses wall time, so the loop yields to both
/// the OS watcher and the async delivery task.
fn wait_for_watch_refresh(
    cx: &mut TestAppContext,
    ws: &gpui::Entity<Workspace>,
    predicate: impl Fn(&Workspace) -> bool,
) {
    let deadline = std::time::Instant::now() + WATCH_REFRESH_TIMEOUT;
    loop {
        ws.update(cx, |ws, cx| ws.drain_git_watchers(cx));
        cx.run_until_parked();
        if ws.read_with(cx, |ws, _| predicate(ws)) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "git-dir watcher did not refresh the workspace before the deadline"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// Fetch's entire visible product is the header's `↓N` — it changes no file
/// in the working tree, and the file watcher skips `.git` paths on purpose,
/// so nothing else re-reads tracking on its behalf. If the op does not
/// refresh the cached tracking itself, a successful fetch leaves the panel
/// byte-identical and reads to the user as a dead button.
#[gpui::test]
fn fetch_refreshes_the_tracking_indicator(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let root = daruda_core::path::canonicalize(temp.path()).unwrap();
    let fake = crate::test_support::fake_git(cx);
    fake.add_repo(FakeRepo::at(&root).branch("main").tracking(behind(0)));

    init_gpui_component(cx);
    let config = daruda_config::Config::default();
    let project = daruda_store::project::Project::from_path(&root);
    let wh = cx.add_window(|window, cx| {
        Workspace::new_with_project_for_test(
            &config,
            Some(project),
            fresh_test_data_dir(),
            window,
            cx,
        )
    });
    let ws = wh.root(cx).unwrap();
    ws.update(cx, |ws, cx| ws.reconcile_bootstrapped_lanes(cx));
    cx.run_until_parked();

    let id = ws.read_with(cx, |ws, _| ws.active_ref());
    ws.update(cx, |ws, cx| ws.refresh_git_status(id, cx));
    cx.run_until_parked();
    ws.read_with(cx, |ws, _| {
        let status = ws.lane_scoped[&id].git.tracking.as_ref().unwrap();
        assert_eq!(status.upstream.as_deref(), Some("origin/main"));
        assert_eq!(status.behind, 0);
    });

    // What the remote holds once fetched; nothing but a re-read can show it.
    fake.set_tracking(&root, behind(1));
    cx.run_until_parked();
    let reads = fake.reads(GitOp::Tracking, &root);
    ws.read_with(cx, |ws, _| assert_eq!(tracking_behind(ws, &id), 0));

    ws.update(cx, |ws, cx| ws.on_fetch(cx));
    cx.run_until_parked();

    assert_eq!(fake.calls(), vec![GitCall::Fetch { repo: root.clone() }]);
    assert!(
        fake.reads(GitOp::Tracking, &root) > reads,
        "the fetch must re-read tracking"
    );
    ws.read_with(cx, |ws, _| {
        assert_eq!(
            tracking_behind(ws, &id),
            1,
            "a successful fetch must leave the header showing what it found"
        );
    });
}

/// A commit made outside daruda changes only git metadata. The dedicated
/// per-worktree watcher must invalidate the linked lane's working-tree cache;
/// the ordinary file-tree watcher cannot see this transition.
#[gpui::test]
fn external_commit_refreshes_linked_lane_through_the_git_dir_watcher(cx: &mut TestAppContext) {
    let staged = GitWorktreeStatus {
        staged: vec![GitFileEntry {
            x: 'A',
            y: ' ',
            path: "side.txt".into(),
            original_path: None,
        }],
        ..GitWorktreeStatus::default()
    };
    let lanes = two_lanes_with(cx, |root, linked| {
        (
            FakeRepo::at(root).branch("main"),
            FakeRepo::at(linked).branch("side").status(staged),
        )
    });

    let (_wh, ws) = build_workspace_with(
        cx,
        &daruda_config::Config::default(),
        Some(daruda_store::project::Project::from_path(&lanes.root)),
    );
    ws.update(cx, |ws, cx| ws.reconcile_bootstrapped_lanes(cx));
    cx.run_until_parked();

    let target = ws.read_with(cx, |ws, _| {
        let project = &ws.projects[0];
        let lane = project
            .lanes
            .iter()
            .find(|lane| lane.path == lanes.linked)
            .expect("linked worktree must be a workspace lane");
        daruda_store::project::LaneRef {
            project: project.id,
            lane: lane.id,
        }
    });
    ws.update(cx, |ws, cx| ws.refresh_git_status(target, cx));
    cx.run_until_parked();
    ws.read_with(cx, |ws, _| {
        assert_eq!(
            ws.lane_git_worktree(target)
                .expect("linked lane status")
                .staged
                .len(),
            1
        );
        assert_eq!(
            ws.git.watch.watched_count(),
            2,
            "the shared dir and linked lane dir must both be watched"
        );
    });

    // The commit lands in git; until the watcher fires, the panel keeps
    // what it last read.
    ws.update(cx, |ws, cx| ws.drain_git_watchers(cx));
    lanes
        .fake
        .set_status(&lanes.linked, GitWorktreeStatus::default());
    cx.run_until_parked();
    ws.read_with(cx, |ws, _| {
        assert_eq!(ws.lane_git_worktree(target).unwrap().staged.len(), 1);
    });

    // What a real commit in a linked lane rewrites (`a_commit_in_a_linked_
    // lane_rewrites_its_own_index` in daruda_project).
    std::fs::write(lanes.side.join("index"), b"after the commit").unwrap();
    wait_for_watch_refresh(cx, &ws, |ws| {
        ws.lane_git_worktree(target)
            .is_some_and(|status| status.staged.is_empty())
    });
}

/// Fetch writes one shared remote-tracking ref. One common-dir watcher must
/// fan that event out so every lane recomputes its own divergence.
#[gpui::test]
fn external_fetch_refreshes_tracking_for_every_lane(cx: &mut TestAppContext) {
    let lanes = two_lanes_with(cx, |root, linked| {
        (
            FakeRepo::at(root).branch("main").tracking(behind(0)),
            FakeRepo::at(linked).branch("side").tracking(GitTracking {
                branch: Some("side".into()),
                ..behind(0)
            }),
        )
    });

    let (_wh, ws) = build_workspace_with(
        cx,
        &daruda_config::Config::default(),
        Some(daruda_store::project::Project::from_path(&lanes.root)),
    );
    ws.update(cx, |ws, cx| ws.reconcile_bootstrapped_lanes(cx));
    cx.run_until_parked();
    let targets = ws.read_with(cx, |ws, _| lane_refs(ws));
    ws.update(cx, |ws, cx| {
        for target in &targets {
            ws.refresh_git_status(*target, cx);
        }
    });
    cx.run_until_parked();
    ws.read_with(cx, |ws, _| {
        assert_eq!(targets.len(), 2, "fixture must expose both lanes");
        assert_eq!(
            ws.git.watch.watched_count(),
            2,
            "one common-dir watcher plus the linked lane's own watcher"
        );
        for target in &targets {
            assert_eq!(tracking_behind(ws, target), 0);
        }
    });

    // The fetch lands in git; until the watcher fires, both lanes keep what
    // they last read.
    ws.update(cx, |ws, cx| ws.drain_git_watchers(cx));
    lanes.fake.set_tracking(&lanes.root, behind(1));
    lanes.fake.set_tracking(
        &lanes.linked,
        GitTracking {
            branch: Some("side".into()),
            ..behind(1)
        },
    );
    cx.run_until_parked();
    ws.read_with(cx, |ws, _| {
        for target in &targets {
            assert_eq!(tracking_behind(ws, target), 0);
        }
    });

    // What a real fetch writes (`a_fetch_moves_the_shared_remote_ref_and_
    // every_lanes_tracking` in daruda_project).
    let remote_ref = lanes.common.join("refs").join("remotes").join("origin");
    std::fs::create_dir_all(&remote_ref).unwrap();
    std::fs::write(remote_ref.join("main"), b"0123456789abcdef\n").unwrap();
    wait_for_watch_refresh(cx, &ws, |ws| {
        targets
            .iter()
            .all(|target| tracking_behind(ws, target) == 1)
    });
}
