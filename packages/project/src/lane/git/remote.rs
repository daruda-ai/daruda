//! Where `git push` sends a branch, read the way git itself decides it, the
//! single-ref fetches that talk to the same remotes, and the local branches
//! those fetches start or move.

use std::path::Path;

use super::{GitError, reject_option_like, run_git};

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
    /// The branch already has an upstream (`branch.<b>.remote` and `.merge`).
    /// A first push must not replace it: in a fork workflow it names the
    /// repository the branch came from, not the one it pushes to.
    pub has_upstream: bool,
    /// The pull request daruda checked this branch out to review, as the
    /// `branch.<name>.daruda-review` key records it: the branch copies
    /// someone else's head, so it has no pull request of its own to push for.
    pub review: Option<String>,
}

/// The `branch.<name>.` variable naming the pull request a review branch
/// was made for. Git config, not a lane record: it describes the branch,
/// lives as long as it does (`git branch -D` drops it), and outlives a lane.
const REVIEW_KEY: &str = "daruda-review";

/// Where a local branch stands against a fetched head.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalBranch {
    /// No `refs/heads/<branch>`.
    Missing,
    /// Every commit on it is in the head: moving it there loses nothing.
    Contained,
    /// It has commits the head lacks, or the head does not resolve.
    Ahead,
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
    let has_upstream = last(&entries, &format!("branch.{branch}.remote")).is_some()
        && last(&entries, &format!("branch.{branch}.merge")).is_some();
    let url = run_git(wt, ["remote", "get-url", "--push", remote.as_str()])?
        .trim()
        .to_owned();
    let review = last(&entries, &format!("branch.{branch}.{REVIEW_KEY}")).map(str::to_owned);
    let tracking_ref = format!("refs/remotes/{remote}/{remote_branch}");
    let published = ref_exists(wt, &tracking_ref)?;
    Ok(Some(PushTarget {
        remote,
        url,
        branch: remote_branch,
        published,
        has_upstream,
        review,
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

/// `git fetch <remote> <refspec>`, from a remote name or URL. An explicit
/// destination keeps the fetched commit addressable afterwards, unlike
/// `FETCH_HEAD`, which any later fetch can overwrite.
pub(crate) fn fetch_ref(wt: &Path, remote: &str, refspec: &str) -> Result<(), GitError> {
    reject_option_like("remote", remote)?;
    reject_option_like("refspec", refspec)?;
    run_git(wt, ["fetch", remote, refspec]).map(|_| ())
}

/// `git remote get-url <remote>`: the fetch URL after `insteadOf`
/// rewriting. `None` for a remote git does not know (exit 2).
pub(crate) fn remote_url(wt: &Path, remote: &str) -> Result<Option<String>, GitError> {
    reject_option_like("remote", remote)?;
    match run_git(wt, ["remote", "get-url", remote]) {
        Ok(url) => Ok(Some(url.trim().to_owned())),
        Err(GitError::Exit { code: Some(2), .. }) => Ok(None),
        Err(e) => Err(e),
    }
}

/// The branches `remote` had when git last fetched it, by name, sorted;
/// its `HEAD` alias left out. Never fetches.
pub(crate) fn remote_branches(wt: &Path, remote: &str) -> Result<Vec<String>, GitError> {
    reject_option_like("remote", remote)?;
    let prefix = format!("refs/remotes/{remote}/");
    let out = run_git(wt, ["for-each-ref", "--format=%(refname)", &prefix])?;
    Ok(parse_remote_branches(&out, &prefix))
}

fn parse_remote_branches(out: &str, prefix: &str) -> Vec<String> {
    let mut names: Vec<String> = out
        .lines()
        .filter_map(|line| line.trim().strip_prefix(prefix))
        .filter(|name| !name.is_empty() && *name != "HEAD")
        .map(str::to_owned)
        .collect();
    names.sort();
    names
}

pub(crate) fn has_local_branch(wt: &Path, branch: &str) -> Result<bool, GitError> {
    ref_exists(wt, &format!("refs/heads/{branch}"))
}

/// Where `branch` stands against `head`, a ref the caller fetched.
pub(crate) fn local_branch(wt: &Path, branch: &str, head: &str) -> Result<LocalBranch, GitError> {
    reject_option_like("branch", branch)?;
    reject_option_like("head", head)?;
    let local = format!("refs/heads/{branch}");
    if !ref_exists(wt, &local)? {
        return Ok(LocalBranch::Missing);
    }
    if !ref_exists(wt, head)? {
        return Ok(LocalBranch::Ahead);
    }
    match run_git(wt, ["merge-base", "--is-ancestor", local.as_str(), head]) {
        Ok(_) => Ok(LocalBranch::Contained),
        Err(GitError::Exit { code: Some(1), .. }) => Ok(LocalBranch::Ahead),
        Err(e) => Err(e),
    }
}

/// `git branch -f --no-track <branch> <head>`: create `branch` at `head` or
/// move it there. Git refuses a branch checked out in any worktree, so no
/// worktree is left on a moved branch. `--no-track` keeps an upstream the
/// branch has and gives a new one none.
pub(crate) fn move_branch(wt: &Path, branch: &str, head: &str) -> Result<(), GitError> {
    reject_option_like("branch", branch)?;
    reject_option_like("head", head)?;
    run_git(wt, ["branch", "-f", "--no-track", branch, head]).map(|_| ())
}

/// Pin a branch's push destination without replacing its fetch upstream.
pub(crate) fn set_push_remote(wt: &Path, branch: &str, remote: &str) -> Result<(), GitError> {
    reject_option_like("branch", branch)?;
    reject_option_like("remote", remote)?;
    let key = format!("branch.{branch}.pushRemote");
    run_git(wt, ["config", "--local", key.as_str(), remote]).map(|_| ())
}

/// The pull request `branch` was checked out to review, if daruda made it.
pub(crate) fn review_mark(wt: &Path, branch: &str) -> Result<Option<String>, GitError> {
    reject_option_like("branch", branch)?;
    let key = format!("branch.{branch}.{REVIEW_KEY}");
    match run_git(wt, ["config", "--get", key.as_str()]) {
        Ok(value) => Ok(Some(value.trim_end_matches('\n').to_owned())),
        // Exit 1 is an unset key.
        Err(GitError::Exit { code: Some(1), .. }) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Record that `branch` reviews the pull request `mark` names.
pub(crate) fn set_review_mark(wt: &Path, branch: &str, mark: &str) -> Result<(), GitError> {
    reject_option_like("branch", branch)?;
    reject_option_like("review mark", mark)?;
    let key = format!("branch.{branch}.{REVIEW_KEY}");
    run_git(wt, ["config", key.as_str(), mark]).map(|_| ())
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
/// only remote. An upstream of `.` (a local branch, which `git push` would
/// push into this same repository) is passed over for a remote; any other
/// name that is not a configured remote, such as a URL, answers `None`, since
/// git would push to it and no repository can be read from it here.
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
        None => match upstream_remote {
            Some(name) if is_remote(&name) => Some(name),
            Some(name) if name != "." => return None,
            _ => None,
        }
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
