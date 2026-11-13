//! Recovery of stale lane entries without deleting another repository.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use daruda_core::path::{canonicalize, is_within, same_path};

use super::{GitError, git_dirs, list_worktrees};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaneRemovalState {
    Registered,
    Missing,
    Unregistered,
}

#[derive(Debug)]
pub enum RemovalError {
    Git(GitError),
    Io { path: PathBuf, source: io::Error },
    Protected(PathBuf),
}

impl std::fmt::Display for RemovalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Git(error) => error.fmt(f),
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Protected(path) => write!(
                f,
                "refusing to delete a repository, registered worktree, or unsafe path: {}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for RemovalError {}

impl From<GitError> for RemovalError {
    fn from(error: GitError) -> Self {
        Self::Git(error)
    }
}

fn io_error(path: &Path, source: io::Error) -> RemovalError {
    RemovalError::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn metadata(path: &Path) -> Result<Option<fs::Metadata>, RemovalError> {
    match fs::symlink_metadata(path) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_error(path, error)),
    }
}

pub(super) fn inspect(repo: &Path, path: &Path) -> Result<LaneRemovalState, RemovalError> {
    if registered(repo, path)? {
        return Ok(LaneRemovalState::Registered);
    }
    Ok(if metadata(path)?.is_some() {
        LaneRemovalState::Unregistered
    } else {
        LaneRemovalState::Missing
    })
}

pub(super) fn registered(repo: &Path, path: &Path) -> Result<bool, GitError> {
    Ok(list_worktrees(repo)?.iter().any(|lane| {
        same_path(&lane.path, path)
            || (lane.path.file_name() == path.file_name()
                && matches!((lane.path.parent(), path.parent()), (Some(a), Some(b)) if same_path(a, b)))
    }))
}

pub(super) fn remove_orphan(repo: &Path, path: &Path) -> Result<(), RemovalError> {
    let protected = || RemovalError::Protected(path.to_path_buf());
    if !path.is_absolute() {
        return Err(protected());
    }
    let worktrees = list_worktrees(repo)?;
    let Some(meta) = metadata(path)? else {
        return Ok(());
    };
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(protected());
    }
    let target = canonicalize(path).map_err(|e| io_error(path, e))?;
    let common = git_dirs(repo)?.common_dir;
    if worktrees.iter().any(|lane| overlaps(&target, &lane.path)) || overlaps(&target, &common) {
        return Err(protected());
    }
    // A path inside another repository can lack its own .git marker.
    // Inspect parents too, instead of treating a failed git probe as absence.
    for parent in target.ancestors().skip(1) {
        if metadata(&parent.join(".git"))?.is_some() || bare_repository(parent)? {
            return Err(protected());
        }
    }
    check_contents(&target, &target, &common)?;
    // Recheck registration after the filesystem walk; the user confirmed
    // an orphan, never a worktree registered while the dialog was open.
    if inspect(repo, path)? != LaneRemovalState::Unregistered {
        return Err(protected());
    }
    let current = metadata(path)?.ok_or_else(protected)?;
    if current.file_type().is_symlink()
        || canonicalize(path).map_err(|e| io_error(path, e))? != target
    {
        return Err(protected());
    }
    fs::remove_dir_all(path).map_err(|e| io_error(path, e))
}

fn overlaps(a: &Path, b: &Path) -> bool {
    is_within(a, b) || is_within(b, a)
}

fn check_contents(dir: &Path, root: &Path, common: &Path) -> Result<(), RemovalError> {
    if bare_repository(dir)? {
        return Err(RemovalError::Protected(dir.to_path_buf()));
    }
    for entry in fs::read_dir(dir).map_err(|e| io_error(dir, e))? {
        let entry = entry.map_err(|e| io_error(dir, e))?;
        let path = entry.path();
        let kind = entry.file_type().map_err(|e| io_error(&path, e))?;
        if entry.file_name() == ".git" {
            if dir == root && kind.is_file() && dangling_link(&path, common)? {
                continue;
            }
            return Err(RemovalError::Protected(path));
        }
        if kind.is_dir() {
            check_contents(&path, root, common)?;
        }
    }
    Ok(())
}

fn bare_repository(dir: &Path) -> Result<bool, RemovalError> {
    Ok(metadata(&dir.join("HEAD"))?.is_some()
        && metadata(&dir.join("objects"))?.is_some()
        && metadata(&dir.join("refs"))?.is_some())
}

fn dangling_link(marker: &Path, common: &Path) -> Result<bool, RemovalError> {
    let text = fs::read_to_string(marker).map_err(|e| io_error(marker, e))?;
    let Some(gitdir) = text.trim().strip_prefix("gitdir: ") else {
        return Ok(false);
    };
    let gitdir = marker.parent().unwrap_or(marker).join(gitdir);
    let expected_parent = common.join("worktrees");
    Ok(gitdir
        .parent()
        .is_some_and(|parent| same_path(parent, &expected_parent))
        && metadata(&gitdir)?.is_none())
}

#[cfg(test)]
mod tests {}
