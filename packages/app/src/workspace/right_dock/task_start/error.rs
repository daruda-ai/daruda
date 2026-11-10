//! Why a task did not start: the log cause and the desktop toast.

use std::path::PathBuf;

use daruda_store::observability::error_report::{ErrorReport, ErrorSeverity};
use daruda_store::tasks::machine::StartFailure;

/// Why a task did not start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TaskStartError {
    RemoteLane,
    NotFound,
    /// Only a Backlog task starts; any other has run already.
    NotBacklog,
    /// The task's project is not open in the window asked to start it.
    ProjectNotOpen,
    NoGitRepo,
    /// Another worktree is being created in the same repository.
    RepoBusy,
    /// The existing lane the task runs in is gone or cannot be read.
    LaneMissing {
        path: PathBuf,
    },
    /// The lane is there, but no pane could be opened in it. Its own spawn
    /// path reported why.
    PaneUnavailable,
    GitAddFailed {
        detail: String,
    },
    /// The pane opened but the prompt never reached it. The task is now
    /// `Error`, and its row says why in the person's words; the control
    /// surface says it in fixed English.
    PromptUndelivered,
    /// The window went away while the worktree was being created.
    WindowClosed,
    /// The task log refused a start's record. Refusing `DispatchRequested`
    /// leaves the task in Backlog with nothing made. Refusing `LanePrepared`
    /// comes after the lane (a new one wrapped in `WorktreeKept`) and a chat
    /// task's pane exist; no failure is recorded against a log refusing
    /// writes, so the task stays Preparing until the next startup's
    /// reconcile settles it as undelivered.
    NotRecorded {
        detail: String,
    },
    /// The agent a control start's card named was removed. Before the start
    /// was recorded, nothing was; after, no pane opened under another agent.
    AgentUnavailable,
    /// `cause`, after git made the task's new worktree: it stays on disk
    /// `at` a lane or an unregistered path, so the caller is told where and
    /// the orchestrator's budget counts it. Built by [`Self::kept_in`] and
    /// [`Self::left_unregistered`].
    WorktreeKept {
        at: KeptWorktree,
        cause: Box<TaskStartError>,
    },
    /// The task's flow is not one its lane can run by that name.
    FlowMissing,
    /// The task's flow could not run as the task describes it, or refused
    /// to start. `shown` says why to the person; `detail` says it in the
    /// fixed English a control answer carries.
    FlowRefused {
        shown: String,
        detail: String,
    },
}

/// Where a failed start's new worktree stays.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum KeptWorktree {
    /// Registered as this lane, so it can be used as it is.
    Lane(daruda_store::project::LaneRef),
    /// On disk at this path with its branch checked out, but no lane: the
    /// project or window closed between `git worktree add` and registering
    /// it. `git worktree prune` keeps it (its directory exists); `git
    /// worktree remove <path>` removes it, leaving the branch.
    Unregistered(PathBuf),
}

impl TaskStartError {
    /// This failure, found once the start's new worktree was registered as
    /// `lane`.
    pub(in crate::workspace) fn kept_in(self, lane: daruda_store::project::LaneRef) -> Self {
        self.kept(KeptWorktree::Lane(lane))
    }

    /// This failure, found after git made the start's new worktree at
    /// `path` but before any lane registered it.
    pub(in crate::workspace) fn left_unregistered(self, path: PathBuf) -> Self {
        self.kept(KeptWorktree::Unregistered(path))
    }

    /// Wrapped once: a failure that already names its worktree keeps the
    /// place it named.
    fn kept(self, at: KeptWorktree) -> Self {
        match self {
            kept @ Self::WorktreeKept { .. } => kept,
            cause => Self::WorktreeKept {
                at,
                cause: Box::new(cause),
            },
        }
    }

    /// The cause the task log records for a start that failed after it was
    /// requested; `None` where no run was recorded to fail. Only a recorded
    /// start asks: a refusal taken before recording reaches
    /// `finish_task_start` with no execution, so its cause is never read.
    pub(super) fn failure(&self) -> Option<StartFailure> {
        Some(match self {
            Self::NotFound => StartFailure::NotFound,
            Self::NotBacklog | Self::NotRecorded { .. } | Self::RemoteLane => return None,
            Self::WorktreeKept { cause, .. } => return cause.failure(),
            Self::AgentUnavailable => StartFailure::AgentMissing,
            Self::ProjectNotOpen => StartFailure::ProjectNotOpen,
            Self::NoGitRepo => StartFailure::NoGitRepo,
            Self::RepoBusy => StartFailure::RepoBusy,
            Self::LaneMissing { .. } => StartFailure::LaneMissing,
            Self::PaneUnavailable => StartFailure::PaneUnavailable,
            Self::GitAddFailed { .. } => StartFailure::GitAddFailed,
            Self::PromptUndelivered => StartFailure::PromptUndelivered,
            Self::WindowClosed => StartFailure::WindowClosed,
            Self::FlowMissing => StartFailure::FlowMissing,
            Self::FlowRefused { .. } => StartFailure::FlowRefused,
        })
    }

    /// The desktop toast for this failure; `None` where the UI already shows
    /// it — a row in `Error`, a spawn failure its own path reported — or
    /// where the Tasks UI cannot ask for it (an unknown or started task).
    pub(super) fn report(&self) -> Option<ErrorReport> {
        use crate::surface::strings::error;
        let (message, severity, dedup) = match self {
            Self::RemoteLane => (
                crate::surface::strings::task::error_remote_lane(),
                ErrorSeverity::Info,
                "tasks.remote_lane",
            ),
            Self::WorktreeKept { at, cause } => {
                let mut report = cause.report()?;
                if let KeptWorktree::Unregistered(path) = at {
                    report.context.insert(
                        "worktree".into(),
                        daruda_store::observability::system_info::redact_home(path),
                    );
                }
                return Some(report);
            }
            Self::NotFound
            | Self::NotBacklog
            | Self::AgentUnavailable
            | Self::PaneUnavailable
            | Self::PromptUndelivered
            | Self::WindowClosed => return None,
            Self::NotRecorded { detail } => {
                return Some(
                    ErrorReport::new(error::tasks_save_failed())
                        .severity(ErrorSeverity::Error)
                        .message(detail.clone())
                        .at(file!(), line!())
                        .dedup("tasks.start.not_recorded")
                        .build(),
                );
            }
            Self::ProjectNotOpen => (
                error::task_project_not_open(),
                ErrorSeverity::Info,
                "tasks.project_not_open",
            ),
            Self::NoGitRepo => (
                error::tasks_require_git_repo(),
                ErrorSeverity::Info,
                "tasks.no_git_repo",
            ),
            Self::RepoBusy => (
                error::lane_create_busy(),
                ErrorSeverity::Info,
                "lane.create.busy",
            ),
            Self::LaneMissing { path } => {
                return Some(
                    ErrorReport::new(error::task_lane_missing())
                        .severity(ErrorSeverity::Error)
                        .at(file!(), line!())
                        .with_context(
                            "path",
                            daruda_store::observability::system_info::redact_home(path),
                        )
                        .dedup("task.start.lane_missing")
                        .build(),
                );
            }
            Self::FlowMissing => (
                crate::surface::strings::task::error_start_flow_missing(),
                ErrorSeverity::Warning,
                "tasks.start.flow_missing",
            ),
            Self::FlowRefused { shown, .. } => {
                return Some(
                    ErrorReport::new(crate::surface::strings::task::error_start_flow_refused())
                        .severity(ErrorSeverity::Warning)
                        .message(shown.clone())
                        .at(file!(), line!())
                        .dedup("tasks.start.flow_refused")
                        .build(),
                );
            }
            Self::GitAddFailed { detail } => {
                return Some(
                    ErrorReport::new(error::lane_create_failed())
                        .severity(ErrorSeverity::Error)
                        .at(file!(), line!())
                        .with_context("detail", detail.clone())
                        .dedup("lane.create")
                        .build(),
                );
            }
        };
        Some(
            ErrorReport::new(message)
                .severity(severity)
                .at(file!(), line!())
                .dedup(dedup)
                .build(),
        )
    }
}
