//! One answer from an agent's CLI, with nothing it can act with: no tools,
//! no session, an empty directory to run in.
//!
//! For text daruda asks for on the user's behalf (a pull request's title
//! and description). An ACP session cannot serve: the protocol lets an
//! agent run a tool without asking, so only the CLI's own switches take
//! its tools away. A domain whose switches are not confirmed has no run.

use std::sync::atomic::AtomicBool;
use std::time::Duration;

use daruda_core::process::{CaptureError, CaptureLimits, run_captured_until_cancelled};
use daruda_store::accounts::AccountRecipeId;

use super::CliCommand;

/// An agent that does not answer within this is not waited on.
const TIMEOUT: Duration = Duration::from_secs(60);
const STDOUT_BYTES: usize = 4 * 1024 * 1024;
const STDERR_BYTES: usize = 64 * 1024;
/// Characters of what the CLI said on failing that are passed on.
const REASON_CHARS: usize = 300;

/// Claude Code, measured on 2.1.326: no built-in tools, no MCP servers, no
/// skills, no user settings (so none of the user's hooks or plugins run,
/// and their `CLAUDE.md` is not read), and no transcript left behind. The prompt arrives on stdin. The account
/// stays signed in without its settings. The two empty values are written
/// `--flag=`: an empty word of its own can be lost on the way through a
/// Windows command shim, and the next switch would be read as the value.
const CLAUDE_ARGS: &[&str] = &[
    "--cli",
    "-p",
    "--tools=",
    "--strict-mcp-config",
    "--no-session-persistence",
    "--disable-slash-commands",
    "--setting-sources=",
    "--output-format",
    "text",
    "--model",
    "sonnet",
];

/// The CLI words that make `recipe`'s agent answer a prompt on stdin with
/// text only, or `None` for a domain whose switches are not confirmed.
pub fn text_only_args(recipe: AccountRecipeId) -> Option<&'static [&'static str]> {
    match recipe {
        AccountRecipeId::Claude => Some(CLAUDE_ARGS),
        AccountRecipeId::Codex => None,
    }
}

/// Why a text run has no answer.
#[derive(Debug, thiserror::Error)]
pub enum TextRunError {
    #[error("could not make a directory to run in: {0}")]
    Directory(std::io::Error),
    #[error("{0}")]
    Run(CaptureError),
    /// The CLI exited unsuccessfully; what it said, cut short.
    #[error("the agent exited with {status}: {reason}")]
    Exit { status: String, reason: String },
    /// Whoever asked stopped waiting.
    #[error("cancelled")]
    Cancelled,
}

/// Run `command` once with `prompt` on its stdin and return what it
/// printed. **Blocking**, for at most 60 seconds; raising `cancelled`
/// ends the CLI and its tree.
///
/// The run's directory is new, empty and removed after, so nothing in a
/// repository is in reach even of a CLI that ignores its switches.
pub fn run_text(
    command: &CliCommand,
    prompt: String,
    cancelled: &AtomicBool,
) -> Result<String, TextRunError> {
    let directory = tempfile::tempdir().map_err(TextRunError::Directory)?;
    let mut cmd = command.command();
    cmd.current_dir(directory.path());
    let limits = CaptureLimits {
        timeout: TIMEOUT,
        stdout: STDOUT_BYTES,
        stderr: STDERR_BYTES,
    };
    let captured = run_captured_until_cancelled(cmd, prompt.into_bytes(), limits, cancelled)
        .map_err(TextRunError::Run)?
        .map_err(|_| TextRunError::Cancelled)?;
    let stdout = String::from_utf8_lossy(&captured.stdout).into_owned();
    if captured.status.success() {
        return Ok(stdout);
    }
    // Claude prints why (not signed in, over its limit) to stdout in print
    // mode; stderr is the fallback.
    let stderr = String::from_utf8_lossy(&captured.stderr);
    let said = if stdout.trim().is_empty() {
        stderr.trim()
    } else {
        stdout.trim()
    };
    Err(TextRunError::Exit {
        status: captured.status.to_string(),
        reason: said.chars().take(REASON_CHARS).collect(),
    })
}
