//! The one way a worktree is made: lock its repository, run the caller's git
//! off the UI thread, release the lock, then continue on the workspace.
//! Every creating surface (create form, task starts, control surface, Pull
//! requests page) goes through it, so they serialize against each other and
//! name an orphaned checkout the same way; each owns its git and registration.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use daruda_store::observability::error_report::{ErrorReport, ErrorSeverity};
use daruda_store::observability::log_writer::LogWriter;
use daruda_store::project::ProjectId;
use gpui::{Context, Task, Window};

use super::CreateWorktreePlan;
use crate::lane::git::{GitRepo, LaneStart};
use crate::workspace::Workspace;

/// Repositories with a worktree being made. Two `git worktree add` runs
/// against one repository can leave a half-created checkout. The set is
/// per-`Workspace`, so it does not serialize across windows.
#[derive(Debug, Default)]
pub(in crate::workspace) struct RepoLocks(HashSet<PathBuf>);

/// Proof that this workspace holds `repo_root`'s lock. Only
/// [`Workspace::release_repo_lock`] frees it; dropping it keeps the lock.
#[must_use = "a repository lock is held until released"]
#[derive(Debug)]
pub(in crate::workspace) struct RepoLock {
    repo_root: PathBuf,
}

impl RepoLock {
    pub(in crate::workspace) fn repo_root(&self) -> &Path {
        &self.repo_root
    }
}

/// What [`Workspace::spawn_worktree_create`] makes, and where to log it if
/// its window closes first.
pub(in crate::workspace) struct WorktreeCreate {
    pub(in crate::workspace) lock: RepoLock,
    pub(in crate::workspace) new_path: PathBuf,
    /// Dedup key of the log line written when the continuation never runs.
    pub(in crate::workspace) dedup: &'static str,
}

/// The continuation never ran: the window or workspace closed while git
/// worked. `left_at` names the checkout git made, which stays on disk
/// registered nowhere — a later attempt at that path finds it there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct CreateAbandoned {
    pub(in crate::workspace) left_at: Option<PathBuf>,
}

/// A create in flight: the continuation's output, or why it never ran.
pub(in crate::workspace) type CreateTask<O> = Task<Result<O, CreateAbandoned>>;

/// `git worktree add -b <branch> [<base>]` for `plan`: the git work of every
/// creating surface except the Pull requests page, which checks out an
/// existing branch.
pub(in crate::workspace) fn add_new_branch(
    plan: &CreateWorktreePlan,
) -> impl FnOnce(&dyn GitRepo, &Path) -> Result<(), String> + Send + 'static {
    let (branch, base) = (plan.branch.clone(), plan.base_ref.clone());
    move |repo, new_path| {
        repo.add_lane(new_path, LaneStart::new_branch(&branch, base.as_deref()))
            .map_err(|e| e.to_string())
    }
}

impl Workspace {
    /// Take `repo_root`'s lock; `None` while another create holds it.
    pub(in crate::workspace) fn lock_repo(&mut self, repo_root: &Path) -> Option<RepoLock> {
        let repo_root = repo_root.to_path_buf();
        self.pending_lane_creates
            .0
            .insert(repo_root.clone())
            .then_some(RepoLock { repo_root })
    }

    pub(in crate::workspace) fn release_repo_lock(&mut self, lock: RepoLock) {
        self.pending_lane_creates.0.remove(&lock.repo_root);
    }

    /// Run `git` on the background executor, then `finish` with its result;
    /// `Ok` means a checkout exists at `create.new_path`. The lock is freed
    /// before `finish` so a half-registered entry cannot block a retry.
    /// Follow-up on another entity (a modal) awaits the returned task
    /// instead of nesting an update inside `finish`.
    pub(in crate::workspace) fn spawn_worktree_create<T, E, O>(
        &mut self,
        create: WorktreeCreate,
        git: impl FnOnce(&dyn GitRepo, &Path) -> Result<T, E> + Send + 'static,
        finish: impl FnOnce(&mut Self, Result<T, E>, &mut Window, &mut Context<Self>) -> O + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> CreateTask<O>
    where
        T: Send + 'static,
        E: Send + 'static,
        O: 'static,
    {
        let WorktreeCreate {
            lock,
            new_path,
            dedup,
        } = create;
        let repo = crate::git_host::GitHost::global(cx).open(lock.repo_root());
        cx.spawn_in(window, async move |ws, cx| {
            let made = cx
                .background_executor()
                .spawn({
                    let new_path = new_path.clone();
                    async move { git(&*repo, &new_path) }
                })
                .await;
            let left_at = made.is_ok().then_some(new_path);
            let finished = ws.update_in(cx, |ws, window, cx| {
                ws.release_repo_lock(lock);
                finish(ws, made, window, cx)
            });
            // The lock and any in-flight state went with the window. A
            // checkout git made is registered nowhere, so the log is the
            // only place it is named.
            finished.map_err(|e| {
                let mut report =
                    ErrorReport::new("Worktree create finished after its workspace closed")
                        .severity(ErrorSeverity::Warning)
                        .at(file!(), line!())
                        .with_context("error", e.to_string())
                        .dedup(dedup);
                if let Some(path) = &left_at {
                    report = report.with_context("path", path.display().to_string());
                }
                LogWriter::log(report.build());
                CreateAbandoned { left_at }
            })
        })
    }

    /// The create form's worktree: a new branch, then a shell in it.
    /// Refused while another create holds the repository; the returned task
    /// answers with the form's error text or success.
    pub(in crate::workspace) fn create_worktree_from_form(
        &mut self,
        plan: CreateWorktreePlan,
        project_id: ProjectId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<CreateTask<Result<(), String>>, String> {
        let lock = self
            .lock_repo(&plan.repo_root)
            .ok_or_else(crate::surface::strings::error::lane_create_busy)?;
        let create = WorktreeCreate {
            lock,
            new_path: plan.new_path.clone(),
            dedup: "lane.create.modal.completion",
        };
        Ok(self.spawn_worktree_create(
            create,
            add_new_branch(&plan),
            move |ws, added, window, cx| {
                added?;
                // The form does not need the new pane's id.
                ws.finalize_terminal_lane(plan, project_id, window, cx)
                    .map(|_pane| ())
            },
            window,
            cx,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext as _, TestAppContext};

    #[gpui::test]
    fn a_repository_is_locked_once_until_released(cx: &mut TestAppContext) {
        let fixture = crate::test_support::workspace_for_control(cx);
        let root = Path::new("/fake/repo");
        fixture.workspace.update(cx, |ws, _| {
            let held = ws.lock_repo(root).expect("free at first");
            assert!(ws.lock_repo(root).is_none(), "a second holder is refused");
            assert!(
                ws.lock_repo(Path::new("/fake/other")).is_some(),
                "another repository is its own lock"
            );
            ws.release_repo_lock(held);
            assert!(ws.lock_repo(root).is_some(), "free once released");
        });
    }

    fn start(
        fixture: &crate::test_support::ControlFixture,
        cx: &mut TestAppContext,
    ) -> CreateTask<bool> {
        cx.update_window(fixture.window.into(), |_, window, cx| {
            fixture.workspace.update(cx, |ws, cx| {
                let create = WorktreeCreate {
                    lock: ws.lock_repo(Path::new("/fake/repo")).expect("free"),
                    new_path: PathBuf::from("/fake/repo-x"),
                    dedup: "test.lane_create",
                };
                ws.spawn_worktree_create(
                    create,
                    |_, _| Ok::<(), ()>(()),
                    // Whether the lock was already free when finish ran.
                    |ws, _, _, _| match ws.lock_repo(Path::new("/fake/repo")) {
                        Some(lock) => {
                            ws.release_repo_lock(lock);
                            true
                        }
                        None => false,
                    },
                    window,
                    cx,
                )
            })
        })
        .expect("window is live")
    }

    #[gpui::test]
    async fn the_lock_is_released_before_the_continuation_runs(cx: &mut TestAppContext) {
        let fixture = crate::test_support::workspace_for_control(cx);
        let task = start(&fixture, cx);
        cx.run_until_parked();
        assert_eq!(task.await, Ok(true));
    }

    #[gpui::test]
    async fn a_window_closed_mid_create_names_the_checkout_left_behind(cx: &mut TestAppContext) {
        let fixture = crate::test_support::workspace_for_control(cx);
        let task = start(&fixture, cx);
        cx.update_window(fixture.window.into(), |_, window, _| window.remove_window())
            .expect("window is live");
        cx.run_until_parked();
        assert_eq!(
            task.await,
            Err(CreateAbandoned {
                left_at: Some(PathBuf::from("/fake/repo-x"))
            })
        );
    }
}
