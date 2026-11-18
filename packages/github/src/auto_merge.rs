//! Turning auto-merge on with the stack below a pull request: the range a
//! person saw must still be the stack's when it is sent.

use crate::{Gh, GhError, MergeMethod, RepoId};

/// Turn auto-merge on for pull request `number` (node id `pr`) at
/// `head_sha`. With `stack`, the range below it the person was shown: the
/// stack is read again first, and nothing is sent
/// ([`GhError::StackChanged`]) when that range is no longer the stack's.
pub fn enable_auto_merge_shown(
    gh: &dyn Gh,
    repo: &RepoId,
    number: u64,
    pr: &str,
    method: MergeMethod,
    head_sha: &str,
    stack: Option<&[u64]>,
) -> Result<(), GhError> {
    if let Some(shown) = stack
        && gh.pr_stack(repo, number)?.range_below(number).as_deref() != Some(shown)
    {
        return Err(GhError::StackChanged);
    }
    gh.enable_auto_merge(repo, pr, method, head_sha, stack.is_some())
}

#[cfg(test)]
mod tests;
