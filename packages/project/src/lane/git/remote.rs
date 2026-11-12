//! Where `git push` sends a branch, read the way git itself decides it.

use std::path::Path;

use super::{GitError, run_git};

/// The remote and remote-side branch the current branch pushes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushTarget {
    /// Remote name, e.g. `origin`.
    pub remote: String,
    /// The remote's push URL after `insteadOf` rewriting.
    pub url: String,
    /// Branch name on the remote side.
    pub branch: String,
    /// Whether `refs/remotes/<remote>/<branch>` exists, i.e. the branch has
    /// been pushed (or fetched) at least once.
    pub published: bool,
}

/// `None` when HEAD is detached or no remote can be chosen.
pub(crate) fn push_target(wt: &Path) -> Result<Option<PushTarget>, GitError> {
    let Some(branch) = current_branch(wt)? else {
        return Ok(None);
    };
    let config = run_git(wt, ["config", "--list", "-z"])?;
    let entries = parse_config_list(&config);
    let Some((remote, remote_branch)) = resolve(&branch, &entries) else {
        return Ok(None);
    };
    let url = run_git(wt, ["remote", "get-url", "--push", remote.as_str()])?
        .trim()
        .to_owned();
    let tracking_ref = format!("refs/remotes/{remote}/{remote_branch}");
    let published = ref_exists(wt, &tracking_ref)?;
    Ok(Some(PushTarget {
        remote,
        url,
        branch: remote_branch,
        published,
    }))
}

/// `git push -u <remote> HEAD:refs/heads/<branch>`.
pub(crate) fn push_set_upstream(wt: &Path, remote: &str, branch: &str) -> Result<(), GitError> {
    let refspec = format!("HEAD:refs/heads/{branch}");
    run_git(wt, ["push", "-u", remote, refspec.as_str()]).map(|_| ())
}

fn current_branch(wt: &Path) -> Result<Option<String>, GitError> {
    match run_git(wt, ["symbolic-ref", "--quiet", "--short", "HEAD"]) {
        Ok(name) => Ok(Some(name.trim().to_owned())),
        // Exit 1 with `--quiet` is a detached HEAD, not a failure.
        Err(GitError::Exit { code: Some(1), .. }) => Ok(None),
        Err(e) => Err(e),
    }
}

fn ref_exists(wt: &Path, name: &str) -> Result<bool, GitError> {
    match run_git(wt, ["rev-parse", "--verify", "--quiet", name]) {
        Ok(_) => Ok(true),
        Err(GitError::Exit { code: Some(1), .. }) => Ok(false),
        Err(e) => Err(e),
    }
}

/// `git config --list -z` records: `key\nvalue\0`, or `key\0` for a bare
/// boolean. Section and variable names arrive lowercased; a subsection
/// (`branch.<name>`, `remote.<name>`) keeps its case.
fn parse_config_list(raw: &str) -> Vec<(&str, &str)> {
    raw.split('\0')
        .filter(|record| !record.is_empty())
        .map(|record| record.split_once('\n').unwrap_or((record, "")))
        .collect()
}

/// The last value of `key`, as git reads a multi-valued key for a single answer.
fn last<'a>(entries: &[(&str, &'a str)], key: &str) -> Option<&'a str> {
    entries
        .iter()
        .rev()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| *v)
}

/// Pick `(remote, remote branch)` for `branch`: `pushRemote`, then
/// `remote.pushDefault`, then the upstream remote, then `origin`, then the
/// only remote. A name that is not a configured remote (`.`, a URL) is skipped.
/// The remote branch is the upstream's when pushing to the upstream remote.
fn resolve(branch: &str, entries: &[(&str, &str)]) -> Option<(String, String)> {
    // A remote with several `url` lines is still one remote.
    let mut remotes: Vec<&str> = entries
        .iter()
        .filter_map(|(k, _)| k.strip_prefix("remote.")?.strip_suffix(".url"))
        .collect();
    remotes.sort_unstable();
    remotes.dedup();
    let is_remote = |name: &&str| remotes.contains(name);
    let upstream_remote = last(entries, &format!("branch.{branch}.remote"));
    let remote = [
        last(entries, &format!("branch.{branch}.pushremote")),
        last(entries, "remote.pushdefault"),
        upstream_remote,
    ]
    .into_iter()
    .flatten()
    .find(is_remote)
    .or_else(|| remotes.iter().copied().find(|r| *r == "origin"))
    .or_else(|| (remotes.len() == 1).then(|| remotes[0]))?;

    let merged = last(entries, &format!("branch.{branch}.merge"))
        .and_then(|merge| merge.strip_prefix("refs/heads/"));
    let remote_branch = match (upstream_remote, merged) {
        (Some(upstream), Some(merged)) if upstream == remote => merged,
        _ => branch,
    };
    Some((remote.to_owned(), remote_branch.to_owned()))
}

#[cfg(test)]
mod tests;
