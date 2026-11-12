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

/// `git push [-u] <remote> HEAD:refs/heads/<branch>`: an explicit refspec,
/// so it pushes where [`push_target`] said whatever `push.default` would do,
/// and is a no-op when the remote branch is already at HEAD.
pub(crate) fn push_head(
    wt: &Path,
    remote: &str,
    branch: &str,
    set_upstream: bool,
) -> Result<(), GitError> {
    let refspec = format!("HEAD:refs/heads/{branch}");
    let mut args = vec!["push"];
    if set_upstream {
        args.push("-u");
    }
    args.extend([remote, refspec.as_str()]);
    run_git(wt, args).map(|_| ())
}

fn current_branch(wt: &Path) -> Result<Option<String>, GitError> {
    // The full name, not `--short`: a tag named like the branch makes the
    // short form `heads/<branch>`.
    match run_git(wt, ["symbolic-ref", "--quiet", "HEAD"]) {
        Ok(name) => Ok(name.trim().strip_prefix("refs/heads/").map(str::to_owned)),
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
/// only remote. An upstream of `.` (a local branch) is skipped; a push remote
/// given as a URL rather than a remote name answers `None`, since no
/// repository can be read from it here.
///
/// The remote branch is the upstream's only under `push.default=upstream`
/// (or its old name `tracking`) when pushing to the upstream remote. Every
/// other mode, `simple` the default among them, pushes to the same name.
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
    let explicit = last(entries, &format!("branch.{branch}.pushremote"))
        .or_else(|| last(entries, "remote.pushdefault"));
    let remote = match explicit {
        Some(name) if is_remote(&name) => name,
        Some(_) => return None,
        None => upstream_remote
            .filter(is_remote)
            .or_else(|| remotes.iter().copied().find(|r| *r == "origin"))
            .or_else(|| (remotes.len() == 1).then(|| remotes[0]))?,
    };

    let follows_upstream = matches!(last(entries, "push.default"), Some("upstream" | "tracking"));
    let merged = last(entries, &format!("branch.{branch}.merge"))
        .and_then(|merge| merge.strip_prefix("refs/heads/"));
    let remote_branch = match (upstream_remote, merged) {
        (Some(upstream), Some(merged)) if follows_upstream && upstream == remote => merged,
        _ => branch,
    };
    Some((remote.to_owned(), remote_branch.to_owned()))
}

#[cfg(test)]
mod tests;
