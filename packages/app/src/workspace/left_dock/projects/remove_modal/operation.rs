//! Blocking removal stages, independent of modal and workspace lifetimes.

use std::path::Path;

use crate::lane::git::{GitRepo, LaneRemovalState};
use crate::surface::strings as s;

pub(super) enum Action {
    Worktree {
        expected: LaneRemovalState,
        delete_folder: bool,
        force: bool,
        branch: Option<String>,
    },
    Branch(String),
}

pub(super) enum Outcome {
    StateChanged(LaneRemovalState),
    Removed { branch_error: Option<String> },
    BranchDeleted,
}

pub(super) fn run(repo: &dyn GitRepo, path: &Path, action: Action) -> Result<Outcome, String> {
    match action {
        Action::Branch(branch) => {
            delete_branch(repo, &branch)?;
            Ok(Outcome::BranchDeleted)
        }
        Action::Worktree {
            expected,
            delete_folder,
            force,
            branch,
        } => {
            let current = repo
                .lane_removal_state(path)
                .map_err(s::modal::remove_lane_check_failed)?;
            if current != expected && current != LaneRemovalState::Missing {
                return Ok(Outcome::StateChanged(current));
            }
            match current {
                LaneRemovalState::Registered => {
                    repo.remove_lane(path, force).map_err(|e| e.to_string())?
                }
                LaneRemovalState::Unregistered if delete_folder => repo
                    .remove_orphaned_lane(path)
                    .map_err(s::modal::remove_lane_folder_failed)?,
                LaneRemovalState::Unregistered | LaneRemovalState::Missing => {}
            }
            // List-only removal preserves the folder and branch together.
            let branch_error = if expected == LaneRemovalState::Unregistered && !delete_folder {
                None
            } else {
                branch.and_then(|b| delete_branch(repo, &b).err())
            };
            Ok(Outcome::Removed { branch_error })
        }
    }
}

fn delete_branch(repo: &dyn GitRepo, branch: &str) -> Result<(), String> {
    let delete = || {
        if repo.has_local_branch(branch)? {
            repo.delete_branch(branch)?;
        }
        Ok::<_, crate::lane::git::GitError>(())
    };
    delete().map_err(|e| s::modal::remove_lane_branch_delete_failed(branch, e))
}

#[cfg(test)]
mod tests;
