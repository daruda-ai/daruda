//! Workspaces over a scripted git: the repository layouts several app tests
//! share, registered with [`FakeGit`] instead of made with real git.

use std::path::{Path, PathBuf};

use gpui::TestAppContext;

use crate::lane::git::{FakeGit, FakeRepo, GitDirs, GitWorktreeInfo};
use crate::workspace::Workspace;

/// A workspace over one scripted repository, its lanes already reconciled
/// so the placeholder lane is a git lane.
pub(in crate::workspace) struct GitFixture {
    pub window: gpui::WindowHandle<gpui_component::Root>,
    pub ws: gpui::Entity<Workspace>,
    pub fake: FakeGit,
    /// The repository's canonical root, the form lanes carry.
    pub root: PathBuf,
    _temp: tempfile::TempDir,
}

impl GitFixture {
    /// A repository on `main`, scripted further by `repo`, and the workspace
    /// over it built with `config`.
    pub(in crate::workspace) fn open(
        cx: &mut TestAppContext,
        config: &daruda_config::Config,
        repo: impl FnOnce(FakeRepo) -> FakeRepo,
    ) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let fake = crate::test_support::fake_git(cx);
        let root = fake_repo_with(&fake, temp.path(), repo);
        let (window, ws) = super::build_workspace_with(
            cx,
            config,
            Some(daruda_store::project::Project::from_path(&root)),
        );
        ws.update(cx, |ws, cx| ws.reconcile_bootstrapped_lanes(cx));
        cx.run_until_parked();
        Self {
            window,
            ws,
            fake,
            root,
            _temp: temp,
        }
    }
}

/// Register a repository on `main` at `<dir>/repo` and return its canonical
/// root, the form real git reports lane paths in. The `repo/` level keeps new
/// lanes inside `dir`: `lane_checkout_path` puts them at `<dir>/repo-<branch>`.
pub(in crate::workspace) fn fake_repo(fake: &FakeGit, dir: &Path) -> PathBuf {
    fake_repo_with(fake, dir, |repo| repo)
}

/// [`fake_repo`], scripted further by `repo`.
pub(in crate::workspace) fn fake_repo_with(
    fake: &FakeGit,
    dir: &Path,
    repo: impl FnOnce(FakeRepo) -> FakeRepo,
) -> PathBuf {
    let dir = dir.join("repo");
    std::fs::create_dir_all(&dir).unwrap();
    let root = daruda_core::path::canonicalize(&dir).unwrap();
    fake.add_repo(repo(FakeRepo::at(&root).branch("main")));
    root
}

/// What `git worktree add` would leave for a lane the app is about to make:
/// its directory, answering as a repository on `branch`. The directory is
/// made here because the fake writes nothing and the app registers the lane
/// without checking it exists; what a test proves is the recorded `AddLane`.
pub(in crate::workspace) fn fake_lane(fake: &FakeGit, path: &Path, branch: &str) {
    std::fs::create_dir_all(path).unwrap();
    fake.add_repo(FakeRepo::at(path).branch(branch));
}

/// A main worktree at `<temp>/work` on `main` and a linked one at
/// `<temp>/side-wt` on `side`, with git's own dir layout (`work/.git` shared,
/// `work/.git/worktrees/side` the linked lane's) as plain directories so the
/// real watchers attach. That real git lays them out so is asserted in
/// `daruda_project`'s
/// `git_dirs_separate_a_linked_worktree_from_the_shared_repository`.
pub(in crate::workspace) struct LinkedWorktree {
    pub fake: FakeGit,
    pub root: PathBuf,
    pub linked: PathBuf,
    /// The shared git dir, `work/.git`.
    pub common: PathBuf,
    /// The linked lane's own git dir, `work/.git/worktrees/side`.
    pub side: PathBuf,
    _temp: tempfile::TempDir,
}

/// Build a [`LinkedWorktree`]. `repos` scripts the two repositories from
/// their roots; the dirs and the lane list are added here.
pub(in crate::workspace) fn fake_linked_worktree(
    cx: &mut TestAppContext,
    repos: impl FnOnce(&Path, &Path) -> (FakeRepo, FakeRepo),
) -> LinkedWorktree {
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
    LinkedWorktree {
        fake,
        root,
        linked,
        common,
        side,
        _temp: temp,
    }
}

fn lane_info(path: &Path, branch: &str) -> GitWorktreeInfo {
    GitWorktreeInfo {
        path: path.to_path_buf(),
        branch: Some(branch.into()),
        head: None,
        bare: false,
    }
}
