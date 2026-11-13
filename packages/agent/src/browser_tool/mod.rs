//! The managed `agent-browser` tool: release selection, digest verification,
//! and the stdio MCP launch each local lane's chat session receives.
//!
//! [`install`] fetches the binary and its browser; ACP wiring belongs to callers.

pub mod install;
mod pinned;

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

pub use pinned::VERSION;

/// Names the per-lane browser session (and its daemon socket).
pub const SESSION_ENV: &str = "AGENT_BROWSER_SESSION";
/// Directory holding the daemon sockets.
pub const SOCKET_DIR_ENV: &str = "AGENT_BROWSER_SOCKET_DIR";
/// Upstream refuses a Unix socket path longer than this many bytes.
pub const UNIX_SOCKET_PATH_LIMIT: usize = 103;

const RELEASE_BASE_URL: &str = "https://github.com/vercel-labs/agent-browser/releases/download";
const SESSION_PREFIX: &str = "daruda-";
const SESSION_HASH_LEN: usize = 12;

/// One single-file release binary, its pinned SHA-256 digest (hex) and its
/// byte size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Asset {
    pub name: &'static str,
    pub sha256: &'static str,
    pub size: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum BrowserToolError {
    #[error("agent-browser {asset} checksum mismatch: expected {expected}, got {actual}")]
    ChecksumMismatch {
        asset: &'static str,
        expected: &'static str,
        actual: String,
    },
}

/// Why a socket directory cannot hold the lanes' session sockets.
#[derive(Debug, thiserror::Error)]
pub enum SocketDirError {
    #[error("agent-browser socket path is {len} bytes, over the {limit}-byte limit")]
    TooLong { len: usize, limit: usize },
    #[error("agent-browser socket directory is not valid UTF-8: {}", .0.display())]
    NonUtf8(PathBuf),
}

/// The release asset for a host, or `None` when upstream ships none.
/// `os`/`arch` use `std::env::consts` values. `musl` is the build target's
/// libc (`cfg!(target_env = "musl")`), not a host probe: a glibc build runs
/// only on a glibc host, and a musl release runs wherever a musl build does.
pub fn target(os: &str, arch: &str, musl: bool) -> Option<Asset> {
    match (os, arch, musl) {
        ("macos", "aarch64", _) => Some(pinned::DARWIN_ARM64),
        ("macos", "x86_64", _) => Some(pinned::DARWIN_X64),
        ("linux", "aarch64", false) => Some(pinned::LINUX_ARM64),
        ("linux", "aarch64", true) => Some(pinned::LINUX_MUSL_ARM64),
        ("linux", "x86_64", false) => Some(pinned::LINUX_X64),
        ("linux", "x86_64", true) => Some(pinned::LINUX_MUSL_X64),
        ("windows", "x86_64", _) => Some(pinned::WIN32_X64),
        _ => None,
    }
}

pub fn download_url(asset: &Asset) -> String {
    format!("{RELEASE_BASE_URL}/{VERSION}/{}", asset.name)
}

pub fn verify(bytes: &[u8], asset: &Asset) -> Result<(), BrowserToolError> {
    let actual = hex::encode(Sha256::digest(bytes));
    if actual.eq_ignore_ascii_case(asset.sha256) {
        Ok(())
    } else {
        Err(BrowserToolError::ChecksumMismatch {
            asset: asset.name,
            expected: asset.sha256,
            actual,
        })
    }
}

/// The installed binary's file name on `os` (a `std::env::consts::OS` value).
pub fn binary_file_name(os: &str) -> &'static str {
    match os {
        "windows" => "agent-browser.exe",
        _ => "agent-browser",
    }
}

/// A stable per-lane session name: `daruda-` plus 12 hex digits of the lane
/// root's SHA-256. The root is canonicalized so two spellings share a session;
/// a vanished root still needs a name, so it then hashes the path as given.
pub fn session_name(lane_root: &Path) -> String {
    let root = daruda_core::path::canonicalize_or_self(lane_root);
    let digest = hex::encode(Sha256::digest(root.as_os_str().as_encoded_bytes()));
    format!("{SESSION_PREFIX}{}", &digest[..SESSION_HASH_LEN])
}

/// How to start `agent-browser mcp` for one lane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpLaunch {
    pub command: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// Whether `socket_dir` can hold every lane's session socket, returning it as
/// the environment value. Session names have a fixed length, so the
/// directory alone decides; [`launch`] applies the same rule.
///
/// `socket_path_limit` is [`UNIX_SOCKET_PATH_LIMIT`] where upstream binds a
/// Unix socket and `None` elsewhere; the caller decides from its platform.
pub fn check_socket_dir(
    socket_dir: &Path,
    socket_path_limit: Option<usize>,
) -> Result<&str, SocketDirError> {
    if let Some(limit) = socket_path_limit {
        let socket = format!("{SESSION_PREFIX}{}.sock", "0".repeat(SESSION_HASH_LEN));
        let len = socket_dir.join(socket).as_os_str().len();
        if len > limit {
            return Err(SocketDirError::TooLong { len, limit });
        }
    }
    socket_dir
        .to_str()
        .ok_or_else(|| SocketDirError::NonUtf8(socket_dir.to_path_buf()))
}

/// Fails only as [`check_socket_dir`] does for the same arguments.
pub fn launch(
    binary: &Path,
    socket_dir: &Path,
    lane_root: &Path,
    socket_path_limit: Option<usize>,
) -> Result<McpLaunch, SocketDirError> {
    let socket_dir_value = check_socket_dir(socket_dir, socket_path_limit)?;
    let session = session_name(lane_root);
    Ok(McpLaunch {
        command: binary.to_path_buf(),
        args: vec!["mcp".to_owned()],
        env: vec![
            (SESSION_ENV.to_owned(), session),
            (SOCKET_DIR_ENV.to_owned(), socket_dir_value.to_owned()),
        ],
    })
}

#[cfg(test)]
mod tests;
