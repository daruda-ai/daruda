//! Path containment shared by flow loading, submission and execution.

use std::path::{Component, Path, PathBuf};

pub(crate) fn stays_relative(path: &Path) -> bool {
    path.components()
        .all(|part| matches!(part, Component::Normal(_) | Component::CurDir))
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum CwdError {
    #[error("could not resolve the working directory: {0}")]
    Io(#[from] std::io::Error),
    #[error("{} resolves outside {}", .cwd.display(), .root.display())]
    Outside { cwd: PathBuf, root: PathBuf },
}

pub(crate) fn check_cwd(root: &Path, cwd: &Path) -> Result<(), CwdError> {
    let root = daruda_core::path::canonicalize(root)?;
    let resolved = daruda_core::path::canonicalize(cwd)?;
    if !daruda_core::path::is_within(&resolved, &root) {
        return Err(CwdError::Outside {
            cwd: resolved,
            root,
        });
    }
    if !resolved.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotADirectory,
            "the working directory is not a directory",
        )
        .into());
    }
    Ok(())
}

/// Why a file named relative to a directory could not be read from under it.
#[derive(Debug)]
pub(crate) enum FileProblem {
    /// Absolute, or climbing with `..`, as written.
    NotRelative(PathBuf),
    Missing(PathBuf),
    /// Inside as written, outside once links are followed.
    Outside {
        path: PathBuf,
        root: PathBuf,
        canonical: PathBuf,
    },
    Unreadable {
        path: PathBuf,
        error: std::io::Error,
    },
}

/// The text of `relative`, read only if it stays under `dir` — as written
/// and once links are followed. The one reading of a flow's prompt and hint
/// files, so a file a flow includes is held to its own folder the same way
/// a file it names directly is.
pub(crate) fn read_under(dir: &Path, relative: &Path) -> Result<String, FileProblem> {
    if !stays_relative(relative) {
        return Err(FileProblem::NotRelative(relative.to_path_buf()));
    }
    let path = dir.join(relative);
    if !path.is_file() {
        return Err(FileProblem::Missing(path));
    }
    let root = daruda_core::path::canonicalize_or_self(dir);
    let canonical = daruda_core::path::canonicalize_or_self(&path);
    if !canonical.starts_with(&root) {
        return Err(FileProblem::Outside {
            path,
            root,
            canonical,
        });
    }
    std::fs::read_to_string(&path).map_err(|error| FileProblem::Unreadable { path, error })
}

#[cfg(test)]
mod tests;
