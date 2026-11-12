//! The installed `gh` CLI.

use std::ffi::OsStr;
use std::process::Stdio;
use std::time::Duration;

use daruda_core::process::{CaptureError, CaptureLimits, Captured, command_on_path, run_captured};

use crate::availability::GhAuth;
use crate::backend::Gh;
use crate::query::{RepoPrs, parse_prs_for_branches, prs_for_branches_query};
use crate::{GhError, NewPr, RepoId};

const LIMITS: CaptureLimits = CaptureLimits {
    timeout: Duration::from_secs(30),
    stdout: 16 * 1024 * 1024,
    stderr: 1024 * 1024,
};

/// Branches per request; the query grows with each one.
const BRANCHES_PER_REQUEST: usize = 50;

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemGh;

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
    fn auth(&self, host: &str) -> Result<GhAuth, GhError> {
        match self.run(["auth", "status", "--hostname", host]) {
            Ok(out) if out.status.success() => Ok(GhAuth::Ready),
            // Exit 1 covers "never logged in" and "token rejected" alike.
            Ok(out) if out.status.code() == Some(1) => Ok(GhAuth::LoggedOut),
            Ok(out) => Err(GhError::Exit {
                code: out.status.code(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            }),
            Err(GhError::NotFound) => Ok(GhAuth::Missing),
            Err(e) => Err(e),
        }
    }

    fn prs_for_branches(&self, repo: &RepoId, branches: &[String]) -> Result<RepoPrs, GhError> {
        let mut all = RepoPrs::default();
        for chunk in branches.chunks(BRANCHES_PER_REQUEST) {
            // `-f`, never `-F`: `-F` would turn a branch or owner named
            // like a number or boolean into one.
            let mut args = vec![
                "api".to_owned(),
                "graphql".to_owned(),
                "--hostname".to_owned(),
                repo.host.clone(),
                "-f".to_owned(),
                format!("query={}", prs_for_branches_query(chunk.len())),
                "-f".to_owned(),
                format!("owner={}", repo.owner),
                "-f".to_owned(),
                format!("name={}", repo.name),
            ];
            for (i, branch) in chunk.iter().enumerate() {
                args.push("-f".to_owned());
                args.push(format!("b{i}={branch}"));
            }
            let json = self.stdout(&args)?;
            let part = parse_prs_for_branches(&json, chunk)?;
            all.default_branch = part.default_branch;
            all.by_branch.extend(part.by_branch);
        }
        Ok(all)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_branches_sends_no_request() {
        let repo = RepoId {
            host: "github.com".into(),
            owner: "o".into(),
            name: "r".into(),
        };
        assert_eq!(
            SystemGh.prs_for_branches(&repo, &[]).unwrap(),
            RepoPrs::default()
        );
    }
}
