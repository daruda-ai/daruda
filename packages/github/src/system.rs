//! The installed `gh` CLI.

use std::ffi::OsStr;
use std::process::Stdio;
use std::time::Duration;

use daruda_core::process::{CaptureError, CaptureLimits, Captured, command_on_path, run_captured};

use crate::availability::{AuthStatus, GhAuth, parse_active_account};
use crate::backend::Gh;
use crate::query::{
    REVIEW_THREADS_QUERY, RepoPrs, newest_first, only_numbers_not_found, paged_search_args,
    parse_graphql_reset, parse_pr_detail, parse_prs_for_branches, parse_review_threads,
    parse_search_page, parse_search_prs, pr_detail_query, prs_for_branches_query, search_prs_query,
    search_string,
};
use crate::search::SEARCH_REPOS_MAX;
use crate::{
    ActionsJob, GhError, MergeMethod, NewPr, PrDetail, PrFilter, PrSearch, PrSearchPage,
    PrSearchRequest, RepoId, ReviewThreads,
};

const LIMITS: CaptureLimits = CaptureLimits {
    timeout: Duration::from_secs(30),
    stdout: 16 * 1024 * 1024,
    stderr: 1024 * 1024,
};

/// Branches, and separately numbers, per request; the query grows with each.
const BRANCHES_PER_REQUEST: usize = 50;

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemGh;

/// Refuse a merge pin that is not a commit SHA: an empty one would make
/// `gh` merge whatever the head is now.
pub(crate) fn check_head_sha(head_sha: &str) -> Result<(), GhError> {
    if !head_sha.is_empty() && head_sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(GhError::BadArgument(format!(
            "merge pinned to {head_sha:?}, not a commit SHA"
        )))
    }
}

/// With a method named and no terminal, `gh pr merge` asks nothing. Never
/// `--delete-branch` (a lane may stand on it) or `--auto` (a merge the user
/// did not watch happen).
// ASSUMPTION: not measured — gh's help (2.74.2) says a branch that requires
// a merge queue gets the pull request enqueued, or auto-merge turned on,
// without `--auto`; that may exit 0 with nothing merged yet.
fn merge_args(
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

fn ready_args(repo: &RepoId, number: u64) -> Vec<String> {
    let repo = repo.qualified();
    ["pr", "ready", &number.to_string(), "--repo", &repo]
        .map(str::to_owned)
        .to_vec()
}

fn close_args(repo: &RepoId, number: u64) -> Vec<String> {
    let repo = repo.qualified();
    ["pr", "close", &number.to_string(), "--repo", &repo]
        .map(str::to_owned)
        .to_vec()
}

fn rerun_failed_args(repo: &RepoId, run: u64) -> Vec<String> {
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

/// The repositories of `repos` one search of `host` names, each once and in
/// the order given. A `repo:` qualifier names a repository on the host
/// asked, so one from another host would list a stranger's repository; a
/// repeated one could land in two chunks and list its pull requests twice.
fn searched_repos(host: &str, repos: &[RepoId]) -> Vec<RepoId> {
    let mut seen = std::collections::HashSet::new();
    repos
        .iter()
        .filter(|repo| repo.host().eq_ignore_ascii_case(host))
        .filter(|repo| seen.insert(*repo))
        .cloned()
        .collect()
}

impl SystemGh {
    fn run<I, S>(&self, args: I) -> Result<Captured, GhError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.run_in(None, args)
    }

    fn run_in<I, S>(&self, cwd: Option<&std::path::Path>, args: I) -> Result<Captured, GhError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = command_on_path("gh", None);
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }
        command
            .args(args)
            // A missing login or unknown host must fail, not wait on a prompt.
            .env("GH_PROMPT_DISABLED", "1")
            .env("NO_COLOR", "1")
            .stdin(Stdio::null());
        match run_captured(command, LIMITS) {
            Ok(out) => Ok(out),
            Err(CaptureError::Spawn(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(GhError::NotFound)
            }
            Err(CaptureError::Spawn(e)) => Err(GhError::Spawn(e)),
            Err(CaptureError::Timeout(d)) => Err(GhError::Timeout(d)),
            Err(CaptureError::Overflow { limit }) => Err(GhError::Overflow { limit }),
        }
    }

    fn stdout<I, S>(&self, args: I) -> Result<String, GhError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.stdout_in(None, args)
    }

    fn stdout_in<I, S>(&self, cwd: Option<&std::path::Path>, args: I) -> Result<String, GhError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let out = self.run_in(cwd, args)?;
        if !out.status.success() {
            return Err(GhError::Exit {
                code: out.status.code(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            });
        }
        String::from_utf8(out.stdout).map_err(|_| GhError::Parse("output is not UTF-8".into()))
    }
}

impl Gh for SystemGh {
    fn auth(&self, host: &str) -> Result<AuthStatus, GhError> {
        match self.run(["auth", "status", "--hostname", host]) {
            Ok(out) if out.status.success() => {
                let output = format!(
                    "{}\n{}",
                    String::from_utf8_lossy(&out.stdout),
                    String::from_utf8_lossy(&out.stderr)
                );
                Ok(AuthStatus {
                    auth: GhAuth::Ready,
                    account: parse_active_account(&output, host),
                })
            }
            // Exit 1 covers no login, a rejected token, and — measured with
            // gh 2.74.2 behind an unreachable proxy — no network, all saying
            // "the token is invalid". `auth token` reads the stored token
            // without the network: with one, the failure may be the network,
            // so it is an error to back off from, not a logout to report.
            Ok(out) if out.status.code() == Some(1) => {
                match self.run(["auth", "token", "--hostname", host]) {
                    Ok(token) if token.status.success() => Err(GhError::Exit {
                        code: Some(1),
                        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
                    }),
                    Ok(_) => Ok(GhAuth::LoggedOut.into()),
                    Err(e) => Err(e),
                }
            }
            Ok(out) => Err(GhError::Exit {
                code: out.status.code(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            }),
            Err(GhError::NotFound) => Ok(GhAuth::Missing.into()),
            Err(e) => Err(e),
        }
    }

    fn prs_for_branches(
        &self,
        repo: &RepoId,
        branches: &[String],
        numbers: &[u64],
    ) -> Result<RepoPrs, GhError> {
        let mut all = RepoPrs::default();
        for (branches, numbers) in lane_requests(branches, numbers) {
            // `-f` for strings, never `-F`: `-F` would turn a branch or owner
            // named like a number or boolean into one.
            let mut args = vec![
                "api".to_owned(),
                "graphql".to_owned(),
                "--hostname".to_owned(),
                repo.host().to_owned(),
                "-f".to_owned(),
                format!(
                    "query={}",
                    prs_for_branches_query(branches.len(), numbers.len())
                ),
                "-f".to_owned(),
                format!("owner={}", repo.owner()),
                "-f".to_owned(),
                format!("name={}", repo.name()),
            ];
            for (i, branch) in branches.iter().enumerate() {
                args.push("-f".to_owned());
                args.push(format!("b{i}={branch}"));
            }
            for (i, number) in numbers.iter().enumerate() {
                // `-F`: a number must reach GraphQL as an Int.
                args.push("-F".to_owned());
                args.push(format!("n{i}={number}"));
            }
            let out = self.run(&args)?;
            let part = read_lane_answer(&out, branches, numbers)?;
            all.default_branch = part.default_branch;
            all.by_branch.extend(part.by_branch);
            all.by_number.extend(part.by_number);
        }
        Ok(all)
    }

    fn search_prs(
        &self,
        host: &str,
        repos: &[RepoId],
        filter: PrFilter,
    ) -> Result<PrSearch, GhError> {
        let repos = searched_repos(host, repos);
        let mut all = PrSearch::default();
        for chunk in repos.chunks(SEARCH_REPOS_MAX) {
            let json = self.stdout([
                "api".to_owned(),
                "graphql".to_owned(),
                "--hostname".to_owned(),
                host.to_owned(),
                "-f".to_owned(),
                format!("query={}", search_prs_query()),
                "-f".to_owned(),
                format!("q={}", search_string(chunk, filter)),
            ])?;
            let part = parse_search_prs(&json, host)?;
            all.items.extend(part.items);
            all.truncated |= part.truncated;
        }
        newest_first(&mut all.items);
        Ok(all)
    }

    fn search_prs_page(&self, request: &PrSearchRequest) -> Result<PrSearchPage, GhError> {
        if request.repos().is_empty() {
            return request.finish_page(Vec::new(), 0, 0, false, None);
        }
        parse_search_page(&self.stdout(paged_search_args(request)?)?, request)
    }

    fn failed_log(&self, repo: &RepoId, job: ActionsJob) -> Result<String, GhError> {
        self.stdout([
            "run".to_owned(),
            "view".to_owned(),
            job.run.to_string(),
            "--repo".to_owned(),
            repo.qualified(),
            "--job".to_owned(),
            job.job.to_string(),
            "--log-failed".to_owned(),
        ])
    }

    fn review_threads(&self, repo: &RepoId, number: u64) -> Result<ReviewThreads, GhError> {
        let json = self.stdout([
            "api".to_owned(),
            "graphql".to_owned(),
            "--hostname".to_owned(),
            repo.host().to_owned(),
            "-f".to_owned(),
            format!("query={REVIEW_THREADS_QUERY}"),
            "-f".to_owned(),
            format!("owner={}", repo.owner()),
            "-f".to_owned(),
            format!("name={}", repo.name()),
            // `-F`: the number must reach GraphQL as an Int.
            "-F".to_owned(),
            format!("number={number}"),
        ])?;
        parse_review_threads(&json)
    }

    fn pr_detail(&self, repo: &RepoId, number: u64) -> Result<PrDetail, GhError> {
        let json = self.stdout([
            "api".to_owned(),
            "graphql".to_owned(),
            "--hostname".to_owned(),
            repo.host().to_owned(),
            "-f".to_owned(),
            format!("query={}", pr_detail_query()),
            "-f".to_owned(),
            format!("owner={}", repo.owner()),
            "-f".to_owned(),
            format!("name={}", repo.name()),
            // `-F`: the number must reach GraphQL as an Int.
            "-F".to_owned(),
            format!("number={number}"),
        ])?;
        parse_pr_detail(&json)
    }

    fn merge_pr(
        &self,
        repo: &RepoId,
        number: u64,
        method: MergeMethod,
        head_sha: &str,
    ) -> Result<(), GhError> {
        let args = merge_args(repo, number, method, head_sha)?;
        self.stdout(&args)
            .map(drop)
            .map_err(GhError::classify_merge)
    }

    fn set_ready(&self, repo: &RepoId, number: u64) -> Result<(), GhError> {
        self.stdout(ready_args(repo, number)).map(drop)
    }

    fn close_pr(&self, repo: &RepoId, number: u64) -> Result<(), GhError> {
        self.stdout(close_args(repo, number)).map(drop)
    }

    fn rerun_failed(&self, repo: &RepoId, run: u64) -> Result<(), GhError> {
        self.stdout(rerun_failed_args(repo, run)).map(drop)
    }

    fn graphql_rate_limit_reset(&self, host: &str) -> Result<Option<i64>, GhError> {
        // Measured with gh 2.74.2: reading `rate_limit` leaves `used` as it was.
        let json = self.stdout(["api", "--hostname", host, "rate_limit"])?;
        parse_graphql_reset(&json)
    }

    fn create_pr(&self, repo: &RepoId, new: &NewPr) -> Result<String, GhError> {
        let mut args = vec![
            "pr".to_owned(),
            "create".to_owned(),
            "--repo".to_owned(),
            repo.qualified(),
            "--head".to_owned(),
            new.head.clone(),
            "--title".to_owned(),
            new.title.clone(),
            "--body".to_owned(),
            new.body.clone(),
        ];
        if let Some(base) = &new.base {
            args.push("--base".to_owned());
            args.push(base.clone());
        }
        if new.draft {
            args.push("--draft".to_owned());
        }
        let out = self.stdout_in(Some(&new.worktree), &args)?;
        // gh prints the new PR's URL as its last line.
        out.lines()
            .rev()
            .map(str::trim)
            .find(|line| line.starts_with("https://"))
            .map(str::to_owned)
            .ok_or_else(|| GhError::Parse(format!("no PR URL in: {}", out.trim())))
    }
}

/// A lane lookup's answer from what `gh` printed. One that failed only
/// because a number names no pull request still carries the rest; any other
/// failure is the request's.
fn read_lane_answer(
    out: &Captured,
    branches: &[String],
    numbers: &[u64],
) -> Result<RepoPrs, GhError> {
    let json = std::str::from_utf8(&out.stdout).unwrap_or_default();
    if out.status.success() {
        return parse_prs_for_branches(json, branches, numbers);
    }
    let failed = GhError::Exit {
        code: out.status.code(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    };
    if !only_numbers_not_found(json) {
        return Err(failed);
    }
    parse_prs_for_branches(json, branches, numbers).map_err(|_| failed)
}

/// The branches and numbers of each request a lane lookup sends: as many
/// requests as the longer list needs, the shorter one riding along.
fn lane_requests<'a>(branches: &'a [String], numbers: &'a [u64]) -> Vec<(&'a [String], &'a [u64])> {
    let branches: Vec<&[String]> = branches.chunks(BRANCHES_PER_REQUEST).collect();
    let numbers: Vec<&[u64]> = numbers.chunks(BRANCHES_PER_REQUEST).collect();
    (0..branches.len().max(numbers.len()))
        .map(|i| {
            (
                branches.get(i).copied().unwrap_or_default(),
                numbers.get(i).copied().unwrap_or_default(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_branches_sends_no_request() {
        let repo = RepoId::new("github.com", "o", "r");
        assert_eq!(
            SystemGh.prs_for_branches(&repo, &[], &[]).unwrap(),
            RepoPrs::default()
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_lookup_is_excused_only_for_numbers_github_cannot_resolve() {
        use std::os::unix::process::ExitStatusExt as _;

        let captured = |code: i32, stdout: &str| Captured {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: stdout.as_bytes().to_vec(),
            stderr: b"gh: Could not resolve".to_vec(),
        };
        let not_found = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/prs_number_not_found.json"),
        )
        .unwrap();
        let branches = ["a".to_owned()];
        let answer = read_lane_answer(&captured(1, &not_found), &branches, &[9, 404])
            .expect("the rest of the answer stands");
        assert_eq!(answer.by_number[&404], None);
        assert!(answer.by_number[&9].is_some());

        // (label, exit code, stdout)
        let failures = [
            ("nothing printed", 1, ""),
            (
                "another error",
                1,
                r#"{"data":null,"errors":[{"type":"RATE_LIMITED","message":"x"}]}"#,
            ),
            ("a login failure prints nothing", 4, ""),
        ];
        for (label, code, stdout) in failures {
            match read_lane_answer(&captured(code, stdout), &branches, &[404]) {
                Err(GhError::Exit { code: got, .. }) => assert_eq!(got, Some(code), "{label}"),
                other => panic!("{label}: expected the exit, got {other:?}"),
            }
        }
    }

    #[test]
    fn numbers_ride_with_branches_and_need_no_branch_to_be_asked() {
        let many: Vec<String> = (0..=BRANCHES_PER_REQUEST)
            .map(|i| format!("b{i}"))
            .collect();
        /// (label, branches, numbers, (branches, numbers) per request)
        type Case<'a> = (&'a str, &'a [String], &'a [u64], Vec<(usize, usize)>);
        let cases: [Case; 4] = [
            ("nothing", &[], &[], vec![]),
            ("numbers alone", &[], &[7, 9], vec![(0, 2)]),
            ("both in one", &many[..2], &[7], vec![(2, 1)]),
            (
                "numbers in the first of two",
                &many,
                &[7],
                vec![(BRANCHES_PER_REQUEST, 1), (1, 0)],
            ),
        ];
        for (label, branches, numbers, expected) in cases {
            let sizes: Vec<(usize, usize)> = lane_requests(branches, numbers)
                .into_iter()
                .map(|(b, n)| (b.len(), n.len()))
                .collect();
            assert_eq!(sizes, expected, "{label}");
        }
    }

    #[test]
    fn no_repositories_send_no_search() {
        let other_host = [RepoId::new("ghe.corp", "o", "r")];
        for (label, repos) in [("none", &[][..]), ("other host only", &other_host[..])] {
            let found = SystemGh.search_prs("github.com", repos, PrFilter::AllOpen);
            assert_eq!(found.unwrap(), PrSearch::default(), "{label}");
        }
    }

    #[test]
    fn a_search_names_each_repository_of_its_host_once() {
        let repo = |host: &str, name: &str| RepoId::new(host, "o", name);
        // (label, repositories asked for, repositories searched)
        let cases = [
            (
                "a repeat is dropped, the first order kept",
                vec![
                    repo("github.com", "b"),
                    repo("github.com", "a"),
                    repo("github.com", "b"),
                ],
                vec![repo("github.com", "b"), repo("github.com", "a")],
            ),
            (
                "a repeat spelled in another case",
                vec![repo("github.com", "a"), repo("GitHub.com", "A")],
                vec![repo("github.com", "a")],
            ),
            (
                "another host's repository is left out",
                vec![repo("ghe.corp", "a"), repo("github.com", "a")],
                vec![repo("github.com", "a")],
            ),
        ];
        for (label, asked, searched) in cases {
            assert_eq!(searched_repos("github.com", &asked), searched, "{label}");
        }
    }

    /// The argv of each mutation; none is ever run from a test.
    #[test]
    fn mutations_build_their_gh_argv() {
        let repo = RepoId::new("GHE.corp", "O", "r");
        let sha = "0123abcdef";
        let merge = |method| merge_args(&repo, 7, method, sha).unwrap();
        let cases: [(&str, Vec<String>, &[&str]); 6] = [
            (
                "squash",
                merge(MergeMethod::Squash),
                &[
                    "pr",
                    "merge",
                    "7",
                    "--repo",
                    "ghe.corp/o/r",
                    "--squash",
                    "--match-head-commit=0123abcdef",
                ],
            ),
            (
                "merge commit",
                merge(MergeMethod::Merge),
                &[
                    "pr",
                    "merge",
                    "7",
                    "--repo",
                    "ghe.corp/o/r",
                    "--merge",
                    "--match-head-commit=0123abcdef",
                ],
            ),
            (
                "rebase",
                merge(MergeMethod::Rebase),
                &[
                    "pr",
                    "merge",
                    "7",
                    "--repo",
                    "ghe.corp/o/r",
                    "--rebase",
                    "--match-head-commit=0123abcdef",
                ],
            ),
            (
                "ready",
                ready_args(&repo, 7),
                &["pr", "ready", "7", "--repo", "ghe.corp/o/r"],
            ),
            (
                "close",
                close_args(&repo, 7),
                &["pr", "close", "7", "--repo", "ghe.corp/o/r"],
            ),
            (
                "rerun failed",
                rerun_failed_args(&repo, 991),
                &["run", "rerun", "991", "--failed", "--repo", "ghe.corp/o/r"],
            ),
        ];
        for (label, args, expected) in cases {
            assert_eq!(args, expected, "{label}");
            for never in ["--delete-branch", "-d", "--auto", "--admin"] {
                assert!(!args.iter().any(|a| a == never), "{label}: {never}");
            }
        }
    }

    #[test]
    fn a_merge_pinned_to_no_commit_is_refused_before_gh_runs() {
        let repo = RepoId::new("github.com", "o", "r");
        for (label, sha) in [("empty", ""), ("a flag", "--admin"), ("not hex", "main")] {
            // Through `SystemGh` itself: a refused pin must not spawn `gh`.
            let refused = SystemGh.merge_pr(&repo, 1, MergeMethod::Squash, sha);
            assert!(
                matches!(refused, Err(GhError::BadArgument(_))),
                "{label}: {refused:?}"
            );
        }
    }
}
