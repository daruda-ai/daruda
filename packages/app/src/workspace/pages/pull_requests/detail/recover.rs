//! Reading a file's diff whole when GitHub's listing left its patch out as
//! too large: both sides read at the commits the pull request's diff is
//! taken between, and diffed here. GPUI-free; runs off the foreground.

use daruda_content::diff::line_diff::unified_diff_text;
use daruda_github::{FileChange, FileText, Gh, GhError, RepoId};

/// Patch text a file read whole may come to: past it, building and
/// drawing its rows on the foreground would stall the window.
pub(in crate::workspace) const RECOVER_PATCH_MAX: usize = 4 * 1024 * 1024;

/// Why a file's diff could not be read whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) enum RecoverFailure {
    /// One side is not text.
    Binary,
    /// One side, or the diff of the two, is larger than is read or drawn.
    TooLarge,
    /// GitHub or `gh` failed, or answered what cannot be the file; what
    /// went wrong.
    Failed(String),
}

/// A failure, and the `gh` error behind it when there was one, to report.
#[derive(Debug)]
pub(in crate::workspace) struct RecoverError {
    pub failure: RecoverFailure,
    pub cause: Option<GhError>,
}

impl RecoverError {
    fn plain(failure: RecoverFailure) -> Self {
        Self {
            failure,
            cause: None,
        }
    }

    fn gh(error: GhError) -> Self {
        Self {
            failure: RecoverFailure::Failed(error.to_string()),
            cause: Some(error),
        }
    }
}

/// The file to read, as the listing named it at `head`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct RecoverTarget {
    pub repo: RepoId,
    /// The branch the pull request merges into.
    pub base_branch: String,
    /// The head commit the listing was read at.
    pub head: String,
    /// The merge base found by an earlier read at this head, if any.
    pub merge_base: Option<String>,
    pub path: String,
    pub change: FileChange,
    /// Lines the listing says changed: a diff that comes out empty
    /// contradicts it.
    pub lines: u32,
}

/// A file's diff, read whole, and the merge base it was taken from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct Recovered {
    pub patch: String,
    pub merge_base: String,
}

/// The unified diff of `target` between the merge base and the head.
pub(in crate::workspace) fn recover_patch(
    gh: &dyn Gh,
    target: &RecoverTarget,
) -> Result<Recovered, RecoverError> {
    let RecoverTarget {
        repo, path, change, ..
    } = target;
    let old_path = match change {
        FileChange::Added => None,
        FileChange::Renamed { from } | FileChange::Copied { from } => Some(from.as_str()),
        _ => Some(path.as_str()),
    };
    let new_path = (!matches!(change, FileChange::Removed)).then_some(path.as_str());
    let merge_base = match &target.merge_base {
        Some(base) => base.clone(),
        None => gh
            .merge_base(repo, &target.base_branch, &target.head)
            .map_err(RecoverError::gh)?,
    };
    let text = |path: Option<&str>, commit: &str| -> Result<String, RecoverError> {
        let Some(path) = path else {
            return Ok(String::new());
        };
        match gh.file_at(repo, path, commit) {
            Ok(FileText::Text(text)) => Ok(text),
            Ok(FileText::Binary) => Err(RecoverError::plain(RecoverFailure::Binary)),
            // The change says this side exists: a missing one is a token
            // that cannot read contents, or a commit GitHub does not have.
            Ok(FileText::Absent) => Err(RecoverError::plain(RecoverFailure::Failed(format!(
                "{path} is not at {commit}"
            )))),
            Err(GhError::Overflow { .. }) => Err(RecoverError::plain(RecoverFailure::TooLarge)),
            Err(error) => Err(RecoverError::gh(error)),
        }
    };
    let old = text(old_path, &merge_base)?;
    let new = text(new_path, &target.head)?;
    let patch = unified_diff_text(&old, &new);
    if patch.len() > RECOVER_PATCH_MAX {
        return Err(RecoverError::plain(RecoverFailure::TooLarge));
    }
    if patch.is_empty() && target.lines > 0 {
        return Err(RecoverError::plain(RecoverFailure::Failed(format!(
            "{path} reads the same at {merge_base} and {}",
            target.head
        ))));
    }
    Ok(Recovered { patch, merge_base })
}

#[cfg(test)]
mod tests;
