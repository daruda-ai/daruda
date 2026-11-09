//! The single place a `git` child process is configured.
//!
//! Every repo-targeting invocation in the app goes through
//! [`git_command`]. The flags it pins are not preferences — each one
//! closes a way the CLI would otherwise corrupt what daruda reads back:
//!
//! - `--no-optional-locks` keeps a read-only `git status` from rewriting
//!   `.git/index` to refresh its stat cache. That write is what a `.git`
//!   watcher would see, re-triggering the status it came from.
//! - `core.fsmonitor=false` stops a repo-supplied hook from running as a
//!   side effect of a status read.
//! - `log.showSignature=false` keeps a signature block out of
//!   `--pretty`/`--format` output the parsers here assume is bare.
//! - `--no-pager` and the `C` locale keep output machine-shaped: several
//!   call sites match on git's own English stderr.
//!
//! Test builds also replace the developer's system and global config with
//! [`isolated_git_env`], so hooks, signing, and templates there cannot change
//! what a real-git test observes.

use std::path::Path;
use std::process::Command;

/// A `git` command rooted at `cwd`, configured for machine reading.
pub fn git_command(cwd: &Path) -> Command {
    let mut command = daruda_core::process::command("git");
    command
        .current_dir(cwd)
        .args(["-c", "core.fsmonitor=false"])
        .args(["-c", "log.showSignature=false"])
        .arg("--no-optional-locks")
        .arg("--no-pager")
        // `LANGUAGE` outranks `LC_ALL`/`LANG` in gettext's resolution
        // order, so it must be cleared rather than overridden.
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env_remove("LANGUAGE");
    #[cfg(any(test, feature = "test-support"))]
    command.envs(isolated_git_env());
    command
}

/// The config every test-build git process reads instead of the developer's.
#[cfg(any(test, feature = "test-support"))]
const ISOLATED_GIT_CONFIG: &str = "\
[user]
\tname = daruda-test
\temail = daruda-test@example.invalid
[init]
\tdefaultBranch = main
[commit]
\tgpgsign = false
[tag]
\tgpgsign = false
";

/// Environment that points git at a fixed test config and away from the
/// system one. For fixtures that spawn `git` without [`git_command`].
///
/// The file is written once per process under a pid-based name, so parallel
/// test binaries never read one another's half-written copy.
#[cfg(any(test, feature = "test-support"))]
pub fn isolated_git_env() -> [(&'static str, &'static std::ffi::OsStr); 2] {
    static GLOBAL: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    let global = GLOBAL.get_or_init(|| {
        let path =
            std::env::temp_dir().join(format!("daruda-test-gitconfig-{}", std::process::id()));
        if let Err(error) = std::fs::write(&path, ISOLATED_GIT_CONFIG) {
            panic!(
                "write the isolated test git config {}: {error}",
                path.display()
            );
        }
        path
    });
    [
        ("GIT_CONFIG_NOSYSTEM", std::ffi::OsStr::new("1")),
        ("GIT_CONFIG_GLOBAL", global.as_os_str()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(dir: &Path, args: &[&str]) -> String {
        let out = git_command(dir).arg("config").args(args).output().unwrap();
        assert!(out.status.success(), "git config {args:?}: {out:?}");
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    /// Whatever the developer's own config says, a test-build git reads the
    /// fixed identity and branch name and never signs.
    #[test]
    fn test_builds_read_only_the_isolated_config() {
        if !super::super::has_git() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            config(dir.path(), &["--global", "--get", "user.name"]),
            "daruda-test"
        );
        assert_eq!(config(dir.path(), &["--get", "init.defaultBranch"]), "main");
        assert_eq!(config(dir.path(), &["--get", "commit.gpgsign"]), "false");
        let origins = config(dir.path(), &["--list", "--show-origin"]);
        let global = isolated_git_env()[1].1.to_string_lossy().into_owned();
        for line in origins.lines() {
            assert!(
                line.starts_with(&format!("file:{global}")) || line.starts_with("command line:"),
                "config leaked in from outside the isolated file: {line}"
            );
        }
    }
}
