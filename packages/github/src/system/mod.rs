//! The installed `gh` CLI.

mod argv;
mod lanes;
mod runner;

pub(crate) use argv::{Graphql, check_head_sha};
use argv::{
    close_args, create_args, edit_reviewers_args, merge_args, ready_args, reopen_args,
    rerun_failed_args,
};
use lanes::{lane_requests, read_lane_answer};

use crate::availability::{AuthStatus, GhAuth, parse_active_account};
use crate::backend::Gh;
use crate::query::{
    RepoPrs, paged_search_args, parse_graphql_reset, parse_pr_detail, parse_search_page,
    pr_detail_query, prs_for_branches_query, read_review_threads, review_replies_query,
    review_threads_query,
};
use crate::{
    ActionsJob, GhError, MergeMethod, NewPr, PrDetail, PrSearchPage, PrSearchRequest, RepoId,
    ReviewThreads,
};

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemGh;

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
            Ok(out) => Err(runner::exit(&out)),
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
            let mut call =
                Graphql::on(repo, &prs_for_branches_query(branches.len(), numbers.len()))
                    .repo(repo);
            for (i, branch) in branches.iter().enumerate() {
                call = call.string(&format!("b{i}"), branch);
            }
            for (i, number) in numbers.iter().enumerate() {
                call = call.int(&format!("n{i}"), *number);
            }
            let out = self.run(call.args())?;
            let part = read_lane_answer(&out, branches, numbers)?;
            all.default_branch = part.default_branch;
            all.by_branch.extend(part.by_branch);
            all.by_number.extend(part.by_number);
        }
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
        read_review_threads(
            |after| {
                let mut call = Graphql::on(repo, &review_threads_query()).pr(repo, number);
                if let Some(after) = after {
                    call = call.string("after", after);
                }
                self.stdout(call.args())
            },
            |thread, after| {
                self.stdout(
                    Graphql::on(repo, &review_replies_query())
                        .string("id", thread)
                        .string("after", after)
                        .args(),
                )
            },
        )
    }

    fn pr_files_page(
        &self,
        repo: &RepoId,
        number: u64,
        page: u32,
    ) -> Result<crate::PrFilesPage, GhError> {
        let json = self.stdout([
            "api".to_owned(),
            "--hostname".to_owned(),
            repo.host().to_owned(),
            format!(
                "repos/{}/{}/pulls/{number}/files?per_page={}&page={page}",
                repo.owner(),
                repo.name(),
                crate::PR_FILES_PER_PAGE
            ),
        ])?;
        crate::files::parse_pr_files_page(&json, page)
    }

    fn merge_base(&self, repo: &RepoId, base: &str, head: &str) -> Result<String, GhError> {
        // GitHub has no merge-base endpoint; a comparison names it. Its
        // files still come with the first page, so only the commit is kept
        // from the answer rather than the whole of it captured.
        let out = self.stdout([
            "api".to_owned(),
            "--hostname".to_owned(),
            repo.host().to_owned(),
            format!(
                "repos/{}/{}/compare/{}...{}?per_page=1",
                repo.owner(),
                repo.name(),
                crate::files::url_path(base),
                crate::files::url_path(head),
            ),
            "--jq".to_owned(),
            ".merge_base_commit.sha".to_owned(),
        ])?;
        crate::files::parse_merge_base(&out)
    }

    fn file_at(&self, repo: &RepoId, path: &str, commit: &str) -> Result<crate::FileText, GhError> {
        let out = self.run([
            "api".to_owned(),
            "--hostname".to_owned(),
            repo.host().to_owned(),
            "-H".to_owned(),
            "Accept: application/vnd.github.raw".to_owned(),
            format!(
                "repos/{}/{}/contents/{}?ref={}",
                repo.owner(),
                repo.name(),
                crate::files::url_path(path),
                crate::files::url_path(commit),
            ),
        ])?;
        if out.status.success() {
            return Ok(crate::FileText::of(out.stdout));
        }
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        // gh says `HTTP 404` for a path the commit does not have.
        if stderr.contains("HTTP 404") {
            return Ok(crate::FileText::Absent);
        }
        Err(GhError::Exit {
            code: out.status.code(),
            stderr,
        })
    }

    fn pr_stack(&self, repo: &RepoId, number: u64) -> Result<crate::StackRead, GhError> {
        let answer = self.stdout(
            Graphql::on(repo, &crate::query::stack_query())
                .pr(repo, number)
                .args(),
        );
        match answer {
            Ok(json) => crate::query::parse_stack(&json),
            Err(GhError::Exit { stderr, .. }) if crate::query::lacks_stacks(&stderr) => {
                Ok(crate::StackRead::Unsupported)
            }
            Err(e) => Err(e),
        }
    }

    fn stack_pr(
        &self,
        repo: &RepoId,
        place: crate::StackPlace,
        number: u64,
    ) -> Result<(), GhError> {
        let (args, body) = crate::query::stack_request(repo, place, number);
        // The answer is the stack; `pr_stack` reads it when it is shown.
        self.stdout_fed(args, body.into_bytes()).map(|_| ())
    }

    fn pr_detail(&self, repo: &RepoId, number: u64) -> Result<PrDetail, GhError> {
        let json = self.stdout(
            Graphql::on(repo, &pr_detail_query())
                .pr(repo, number)
                .args(),
        )?;
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

    fn reply_to_thread(
        &self,
        repo: &RepoId,
        thread: &str,
        body: &str,
    ) -> Result<crate::ReviewComment, GhError> {
        // The request goes on stdin: a reply can be longer than an
        // argument may be.
        let request = serde_json::json!({
            "query": crate::query::REPLY_MUTATION,
            "variables": { "thread": thread, "body": body },
        });
        let json = self.stdout_fed(
            ["api", "graphql", "--hostname", repo.host(), "--input", "-"],
            request.to_string().into_bytes(),
        )?;
        crate::query::parse_reply(&json)
    }

    fn set_thread_resolved(
        &self,
        repo: &RepoId,
        thread: &str,
        resolved: bool,
    ) -> Result<(), GhError> {
        let mutation = if resolved {
            crate::query::RESOLVE_MUTATION
        } else {
            crate::query::UNRESOLVE_MUTATION
        };
        let json = self.stdout(Graphql::on(repo, mutation).string("thread", thread).args())?;
        crate::query::parse_resolved(&json, resolved)
    }

    fn pr_conversation(&self, repo: &RepoId, number: u64) -> Result<crate::Conversation, GhError> {
        crate::query::read_conversation(|after| {
            let mut call = Graphql::on(repo, crate::query::CONVERSATION_QUERY).pr(repo, number);
            if let Some(after) = after {
                call = call.string("after", after);
            }
            let args = call.args();
            self.stdout(args)
        })
    }

    fn edit_pr_reviewers(
        &self,
        repo: &RepoId,
        number: u64,
        add: &[String],
        remove: &[String],
    ) -> Result<(), GhError> {
        self.stdout(edit_reviewers_args(repo, number, add, remove)?)
            .map(drop)
    }

    fn submit_review(
        &self,
        repo: &RepoId,
        pr: &str,
        commit: &str,
        event: crate::ReviewEvent,
        body: &str,
        threads: &[crate::ReviewThreadDraft],
    ) -> Result<crate::SubmittedReview, GhError> {
        // On stdin: a review's text can be longer than an argument may be.
        let request = crate::query::submit_request(pr, commit, event, body, threads);
        let json = self.stdout_fed(
            ["api", "graphql", "--hostname", repo.host(), "--input", "-"],
            request.to_string().into_bytes(),
        )?;
        crate::query::parse_submitted(&json)
    }

    fn viewer_latest_review(
        &self,
        repo: &RepoId,
        number: u64,
    ) -> Result<Option<crate::LatestReview>, GhError> {
        let json = self.stdout(
            Graphql::on(repo, crate::query::LATEST_REVIEW_QUERY)
                .pr(repo, number)
                .args(),
        )?;
        crate::query::parse_latest_review(&json)
    }

    fn edit_pr_body(&self, repo: &RepoId, number: u64, body: &str) -> Result<(), GhError> {
        // On stdin: a description can be longer than an argument may be.
        self.stdout_fed(
            [
                "pr".to_owned(),
                "edit".to_owned(),
                number.to_string(),
                "--repo".to_owned(),
                repo.qualified(),
                "--body-file".to_owned(),
                "-".to_owned(),
            ],
            body.as_bytes().to_vec(),
        )
        .map(drop)
    }

    fn close_pr(&self, repo: &RepoId, number: u64) -> Result<(), GhError> {
        self.stdout(close_args(repo, number)).map(drop)
    }

    fn reopen_pr(&self, repo: &RepoId, number: u64) -> Result<(), GhError> {
        self.stdout(reopen_args(repo, number)).map(drop)
    }

    fn enable_auto_merge(
        &self,
        repo: &RepoId,
        pr: &str,
        method: MergeMethod,
        head_sha: &str,
        include_stack: bool,
    ) -> Result<(), GhError> {
        check_head_sha(head_sha)?;
        let json = self.stdout(
            Graphql::on(repo, crate::query::ENABLE_AUTO_MERGE_MUTATION)
                .string("pr", pr)
                .string("method", crate::query::method_name(method))
                .string("head", head_sha)
                .boolean("stack", include_stack)
                .args(),
        )?;
        crate::query::parse_auto_merge(&json, true)
    }

    fn disable_auto_merge(&self, repo: &RepoId, pr: &str) -> Result<(), GhError> {
        let json = self.stdout(
            Graphql::on(repo, crate::query::DISABLE_AUTO_MERGE_MUTATION)
                .string("pr", pr)
                .args(),
        )?;
        crate::query::parse_auto_merge(&json, false)
    }

    fn dequeue_pr(&self, repo: &RepoId, pr: &str) -> Result<(), GhError> {
        let json = self.stdout(
            Graphql::on(repo, crate::query::DEQUEUE_MUTATION)
                .string("pr", pr)
                .args(),
        )?;
        crate::query::parse_dequeue(&json)
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
        // The description on stdin: it can be longer than an argument may be.
        let ran = self.run_fed(
            Some(&new.worktree),
            create_args(repo, new),
            Some(new.body.clone().into_bytes()),
        )?;
        let out = runner::answer(ran)?;
        // gh prints the new PR's URL as its last line.
        out.lines()
            .rev()
            .map(str::trim)
            .find(|line| line.starts_with("https://"))
            .map(str::to_owned)
            .ok_or_else(|| GhError::Parse(format!("no PR URL in: {}", out.trim())))
    }
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

    #[test]
    fn a_bad_reviewer_change_is_refused_before_gh_runs() {
        let repo = RepoId::new("github.com", "o", "r");
        let one = |name: &str| vec![name.to_owned()];
        // (label, add, remove)
        let cases = [
            ("a flag", one("--admin"), Vec::new()),
            ("a space", Vec::new(), one("a b")),
            ("nothing", Vec::new(), Vec::new()),
        ];
        for (label, add, remove) in cases {
            // Through `SystemGh` itself: a refused change must not spawn `gh`.
            let refused = SystemGh.edit_pr_reviewers(&repo, 1, &add, &remove);
            assert!(
                matches!(refused, Err(GhError::BadArgument(_))),
                "{label}: {refused:?}"
            );
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
