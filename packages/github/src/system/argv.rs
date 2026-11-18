//! The argv of each `gh` command that is not a plain read.

use crate::{GhError, MergeMethod, NewPr, RepoId};

/// Refuse a merge pin that is not a commit SHA: an empty one would make
/// `gh` merge whatever the head is now.
pub(crate) fn check_head_sha(head_sha: &str) -> Result<(), GhError> {
    if !head_sha.is_empty() && head_sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(GhError::BadArgument(format!(
            "pinned to {head_sha:?}, not a commit SHA"
        )))
    }
}

/// With a method named and no terminal, `gh pr merge` asks nothing. Never
/// `--delete-branch` (a lane may stand on it) or `--auto` (a merge the user
/// did not watch happen).
// ASSUMPTION: not measured — gh's help (2.74.2) says a branch that requires
// a merge queue gets the pull request enqueued, or auto-merge turned on,
// without `--auto`; that may exit 0 with nothing merged yet.
pub(super) fn merge_args(
    repo: &RepoId,
    number: u64,
    method: MergeMethod,
    head_sha: &str,
) -> Result<Vec<String>, GhError> {
    check_head_sha(head_sha)?;
    let method = match method {
        MergeMethod::Merge => "--merge",
        MergeMethod::Squash => "--squash",
        MergeMethod::Rebase => "--rebase",
    };
    Ok(vec![
        "pr".to_owned(),
        "merge".to_owned(),
        number.to_string(),
        "--repo".to_owned(),
        repo.qualified(),
        method.to_owned(),
        format!("--match-head-commit={head_sha}"),
    ])
}

pub(super) fn ready_args(repo: &RepoId, number: u64) -> Vec<String> {
    let repo = repo.qualified();
    ["pr", "ready", &number.to_string(), "--repo", &repo]
        .map(str::to_owned)
        .to_vec()
}

pub(super) fn close_args(repo: &RepoId, number: u64) -> Vec<String> {
    let repo = repo.qualified();
    ["pr", "close", &number.to_string(), "--repo", &repo]
        .map(str::to_owned)
        .to_vec()
}

/// `gh api graphql` with typed variables: a string goes as `-f`, so a
/// branch named like a number or `true` stays a string; only an Int or a
/// Boolean goes as `-F`, which GraphQL reads as one.
pub(crate) struct Graphql(Vec<String>);

impl Graphql {
    pub(crate) fn new(host: &str, query: &str) -> Self {
        Self(vec![
            "api".to_owned(),
            "graphql".to_owned(),
            "--hostname".to_owned(),
            host.to_owned(),
            "-f".to_owned(),
            format!("query={query}"),
        ])
    }

    /// The same call for `repo`'s host.
    pub(crate) fn on(repo: &RepoId, query: &str) -> Self {
        Self::new(repo.host(), query)
    }

    pub(crate) fn string(mut self, name: &str, value: &str) -> Self {
        self.0.extend(["-f".to_owned(), format!("{name}={value}")]);
        self
    }

    pub(crate) fn int(mut self, name: &str, value: u64) -> Self {
        self.0.extend(["-F".to_owned(), format!("{name}={value}")]);
        self
    }

    pub(crate) fn boolean(mut self, name: &str, value: bool) -> Self {
        self.0.extend(["-F".to_owned(), format!("{name}={value}")]);
        self
    }

    /// `$owner` and `$name`: `repo` itself.
    pub(crate) fn repo(self, repo: &RepoId) -> Self {
        self.string("owner", repo.owner())
            .string("name", repo.name())
    }

    /// `$owner`, `$name` and `$number`: pull request `number` of `repo`.
    pub(crate) fn pr(self, repo: &RepoId, number: u64) -> Self {
        self.repo(repo).int("number", number)
    }

    pub(crate) fn args(self) -> Vec<String> {
        self.0
    }
}

/// `gh pr create` for `new`, its description read from stdin.
pub(super) fn create_args(repo: &RepoId, new: &NewPr) -> Vec<String> {
    let mut args = vec![
        "pr".to_owned(),
        "create".to_owned(),
        "--repo".to_owned(),
        repo.qualified(),
        "--head".to_owned(),
        new.head.clone(),
        "--title".to_owned(),
        new.title.clone(),
        "--body-file".to_owned(),
        "-".to_owned(),
    ];
    if let Some(base) = &new.base {
        args.push("--base".to_owned());
        args.push(base.clone());
    }
    if new.draft {
        args.push("--draft".to_owned());
    }
    args
}

pub(super) fn reopen_args(repo: &RepoId, number: u64) -> Vec<String> {
    let repo = repo.qualified();
    ["pr", "reopen", &number.to_string(), "--repo", &repo]
        .map(str::to_owned)
        .to_vec()
}

/// `gh pr edit` asking `add` and unasking `remove`; refused before `gh`
/// runs for a name that cannot be a reviewer, or for nothing to change.
pub(super) fn edit_reviewers_args(
    repo: &RepoId,
    number: u64,
    add: &[String],
    remove: &[String],
) -> Result<Vec<String>, GhError> {
    if let Some(bad) = add
        .iter()
        .chain(remove)
        .find(|name| !crate::reviewer_name_ok(name))
    {
        return Err(GhError::BadArgument(format!(
            "not a reviewer name: {bad:?}"
        )));
    }
    if add.is_empty() && remove.is_empty() {
        return Err(GhError::BadArgument("no reviewer to change".into()));
    }
    let mut args = vec![
        "pr".to_owned(),
        "edit".to_owned(),
        number.to_string(),
        "--repo".to_owned(),
        repo.qualified(),
    ];
    // Joined with `=`: a name is never read as a flag of its own.
    if !add.is_empty() {
        args.push(format!("--add-reviewer={}", add.join(",")));
    }
    if !remove.is_empty() {
        args.push(format!("--remove-reviewer={}", remove.join(",")));
    }
    Ok(args)
}

pub(super) fn rerun_failed_args(repo: &RepoId, run: u64) -> Vec<String> {
    let repo = repo.qualified();
    [
        "run",
        "rerun",
        &run.to_string(),
        "--failed",
        "--repo",
        &repo,
    ]
    .map(str::to_owned)
    .to_vec()
}

#[cfg(test)]
mod tests;
