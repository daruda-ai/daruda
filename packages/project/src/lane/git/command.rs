//! The single place a `git` child process is configured: every
//! repo-targeting invocation goes through [`git_command`], and each pinned
//! flag closes a way the CLI would otherwise corrupt what daruda reads back.

use std::path::Path;
use std::process::Command;

/// A `git` command rooted at `cwd`, configured for machine reading.
pub(crate) fn git_command(cwd: &Path) -> Command {
    let mut command = daruda_core::process::command("git");
    command
        .current_dir(cwd)
        // A repo-supplied fsmonitor hook would run as a side effect of a read.
        .args(["-c", "core.fsmonitor=false"])
        // A signature block would break the bare `--pretty`/`--format` parsers.
        .args(["-c", "log.showSignature=false"])
        // Keeps a read-only `git status` from rewriting `.git/index`, which a
        // `.git` watcher would see as a change. `diff HEAD --numstat` still
        // refreshes a stat-stale index.
        .arg("--no-optional-locks")
        .arg("--no-pager")
        // Several call sites match git's English stderr. `LANGUAGE` outranks
        // `LC_ALL`/`LANG` in gettext's order, so it is cleared, not overridden.
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env_remove("LANGUAGE");
    #[cfg(any(test, feature = "test-support"))]
    isolate(&mut command, isolation_on());
    command
}

/// Set only by [`isolate_git_config_for_tests`]. A `test-support` build that
/// never calls it, such as the app binary `cargo test` links, runs git exactly
/// as production does.
#[cfg(any(test, feature = "test-support"))]
static ISOLATE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// This crate's own unit-test binary never ships, so it is isolated
/// unconditionally rather than relying on each test to opt in.
#[cfg(any(test, feature = "test-support"))]
fn isolation_on() -> bool {
    cfg!(test) || ISOLATE.load(std::sync::atomic::Ordering::Acquire)
}

#[cfg(any(test, feature = "test-support"))]
fn isolate(command: &mut Command, on: bool) {
    if on {
        command.envs(isolated_git_env());
    }
}

/// Points every later git process at a fixed test config instead of the
/// developer's system and global config, so their hooks, signing, and
/// templates cannot change what a real-git test observes. Idempotent.
///
/// Returns that environment for fixtures that spawn `git` themselves.
#[cfg(any(test, feature = "test-support"))]
pub fn isolate_git_config_for_tests() -> [(&'static str, &'static std::ffi::OsStr); 2] {
    ISOLATE.store(true, std::sync::atomic::Ordering::Release);
    isolated_git_env()
}

/// The config every isolated git process reads instead of the developer's.
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

#[cfg(any(test, feature = "test-support"))]
fn isolated_git_env() -> [(&'static str, &'static std::ffi::OsStr); 2] {
    static GLOBAL: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    let global = GLOBAL.get_or_init(|| {
        // The name carries a version so a changed config never reuses a stale
        // file; bump it with `ISOLATED_GIT_CONFIG`.
        let path = std::env::temp_dir().join(isolated_config_name());
        if let Err(error) = write_isolated_config(&path) {
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

/// The shared temp directory can hold other users' files, so the name is
/// scoped per effective user where the OS has one.
#[cfg(any(test, feature = "test-support"))]
fn isolated_config_name() -> String {
    const BASE: &str = "daruda-test-gitconfig-v1";
    match effective_user() {
        Some(uid) => format!("{BASE}-{uid}"),
        None => BASE.to_owned(),
    }
}

#[cfg(all(unix, any(test, feature = "test-support")))]
fn effective_user() -> Option<u32> {
    // SAFETY: `geteuid` takes no arguments, cannot fail, and has no
    // memory-safety preconditions.
    Some(unsafe { libc::geteuid() })
}

#[cfg(all(not(unix), any(test, feature = "test-support")))]
fn effective_user() -> Option<u32> {
    None
}

/// Shared by every test binary, so it is published by rename: a reader never
/// sees a half-written copy. A target already holding the bytes is success,
/// which also covers a rename refused because another process has it open.
#[cfg(any(test, feature = "test-support"))]
fn write_isolated_config(path: &Path) -> std::io::Result<()> {
    let current = || std::fs::read(path).is_ok_and(|bytes| bytes == ISOLATED_GIT_CONFIG.as_bytes());
    if current() {
        return Ok(());
    }
    let mut temp = path.as_os_str().to_owned();
    temp.push(format!(".{}.tmp", std::process::id()));
    let temp = std::path::PathBuf::from(temp);
    std::fs::write(&temp, ISOLATED_GIT_CONFIG)?;
    let renamed = std::fs::rename(&temp, path);
    // SILENT-OK: the temp file only lingers if the rename failed, and a
    // leftover in the temp directory is harmless.
    let _ = std::fs::remove_file(&temp);
    match renamed {
        Err(_) if current() => Ok(()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_env(command: &Command) -> Vec<String> {
        command
            .get_envs()
            .map(|(key, _)| key.to_string_lossy().into_owned())
            .filter(|key| key.starts_with("GIT_CONFIG"))
            .collect()
    }

    /// Without the switch a `test-support` build adds nothing to git's
    /// environment: it reads the developer's config like production.
    #[test]
    fn without_the_switch_git_reads_the_normal_config() {
        let mut command = Command::new("git");
        isolate(&mut command, false);
        assert!(config_env(&command).is_empty());
    }

    #[test]
    fn with_the_switch_git_reads_only_the_isolated_config() {
        let mut command = Command::new("git");
        isolate(&mut command, true);
        let mut keys = config_env(&command);
        keys.sort();
        assert_eq!(keys, ["GIT_CONFIG_GLOBAL", "GIT_CONFIG_NOSYSTEM"]);
    }

    #[test]
    fn the_config_file_is_rewritten_only_when_its_bytes_differ() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gitconfig");
        std::fs::write(&path, "stale").unwrap();
        write_isolated_config(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), ISOLATED_GIT_CONFIG);
        write_isolated_config(&path).unwrap();
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    fn config(dir: &Path, args: &[&str]) -> Vec<u8> {
        let out = git_command(dir).arg("config").args(args).output().unwrap();
        assert!(out.status.success(), "git config {args:?}: {out:?}");
        out.stdout
    }

    fn config_value(dir: &Path, args: &[&str]) -> String {
        String::from_utf8(config(dir, args))
            .unwrap()
            .trim()
            .to_owned()
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
            config_value(dir.path(), &["--global", "--get", "user.name"]),
            "daruda-test"
        );
        assert_eq!(
            config_value(dir.path(), &["--get", "init.defaultBranch"]),
            "main"
        );
        assert_eq!(
            config_value(dir.path(), &["--get", "commit.gpgsign"]),
            "false"
        );
        // With `-z` each entry is `<origin>\0<key>\n<value>\0` and the origin
        // path is printed unquoted, so it compares verbatim on every OS.
        let listing = config(dir.path(), &["--list", "--show-origin", "-z"]);
        let global = format!("file:{}", isolated_git_env()[1].1.to_string_lossy());
        let fields: Vec<&[u8]> = listing.split(|byte| *byte == 0).collect();
        let origins = fields.chunks(2).filter(|entry| entry.len() == 2);
        let mut seen = 0;
        for entry in origins {
            let origin = String::from_utf8_lossy(entry[0]);
            assert!(
                origin == global || origin == "command line:",
                "config leaked in from outside the isolated file: {origin}"
            );
            seen += 1;
        }
        assert!(seen > 0, "no config entries listed: {listing:?}");
    }
}
