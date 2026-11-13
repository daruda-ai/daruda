//! Installs the pinned agent-browser binary and the browser it drives.
//!
//! Layout: `<tool_dir>/<VERSION>/<binary>` plus a `.complete` marker that
//! appears only once the browser install has succeeded. Callers serialize
//! installs within one process; upstream's browser extractor takes no lock,
//! so installs from two processes (profiles) can race over the browser.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use daruda_core::process::{CaptureError, CaptureLimits, Captured, run_captured};

use super::{Asset, BrowserToolError, VERSION, binary_file_name, download_url, verify};

const MARKER: &str = ".complete";
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);
const DOWNLOAD_IO_TIMEOUT: Duration = Duration::from_secs(5);
/// Release binaries are ~20 MB; the cap bounds a misbehaving proxy.
const MAX_DOWNLOAD_BYTES: u64 = 256 * 1024 * 1024;
const STDERR_TAIL_CHARS: usize = 2000;
/// The browser step downloads ~188 MB of Chrome: room for a slow link, while
/// a stalled download still ends. The output caps only bound a runaway child.
const BROWSER_INSTALL_LIMITS: CaptureLimits = CaptureLimits {
    timeout: Duration::from_secs(15 * 60),
    stdout: 4 * 1024 * 1024,
    stderr: 4 * 1024 * 1024,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallStage {
    Binary,
    Browser,
}

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("agent-browser download failed: {0}")]
    Download(String),
    #[error(transparent)]
    Verify(#[from] BrowserToolError),
    #[error("agent-browser install I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("agent-browser browser install failed: {0}")]
    Browser(String),
}

pub trait Fetch {
    fn get(&self, url: &str) -> Result<Vec<u8>, InstallError>;
}

/// Runs `<binary> install`, which fetches the browser into the user's home.
pub trait Run {
    fn run_install(&self, binary: &Path) -> Result<(), InstallError>;
}

pub struct HttpFetch;

impl Fetch for HttpFetch {
    fn get(&self, url: &str) -> Result<Vec<u8>, InstallError> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(DOWNLOAD_TIMEOUT))
            .timeout_connect(Some(DOWNLOAD_IO_TIMEOUT))
            .timeout_recv_response(Some(DOWNLOAD_IO_TIMEOUT))
            .timeout_per_read(Some(DOWNLOAD_IO_TIMEOUT))
            .proxy(None)
            .build()
            .into();
        let response = agent
            .get(url)
            .call()
            .map_err(|e| InstallError::Download(e.to_string()))?;
        read_capped(response.into_body().into_reader(), MAX_DOWNLOAD_BYTES)
    }
}

/// Reads all of `reader`, refusing a body over `max` bytes rather than
/// truncating it into a checksum mismatch.
fn read_capped(reader: impl std::io::Read, max: u64) -> Result<Vec<u8>, InstallError> {
    let mut bytes = Vec::new();
    reader
        .take(max.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|e| InstallError::Download(e.to_string()))?;
    if bytes.len() as u64 > max {
        return Err(InstallError::Download(format!(
            "response exceeds {max} bytes"
        )));
    }
    Ok(bytes)
}

pub struct ProcessRun;

impl Run for ProcessRun {
    fn run_install(&self, binary: &Path) -> Result<(), InstallError> {
        let mut command = daruda_core::process::command(binary);
        command.arg("install").stdin(Stdio::null());
        browser_install_outcome(run_captured(command, BROWSER_INSTALL_LIMITS))
    }
}

/// A timeout or an output overflow has already killed the child's tree.
fn browser_install_outcome(result: Result<Captured, CaptureError>) -> Result<(), InstallError> {
    let output = result.map_err(|e| InstallError::Browser(e.to_string()))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(InstallError::Browser(format!(
        "{}: {}",
        output.status,
        tail(stderr.trim(), STDERR_TAIL_CHARS)
    )))
}

fn tail(text: &str, max_chars: usize) -> &str {
    let skip = text.chars().count().saturating_sub(max_chars);
    text.char_indices()
        .nth(skip)
        .map_or("", |(index, _)| &text[index..])
}

/// The installed binary, when the marker exists and the binary has the
/// pinned size. Cheap enough for the UI thread: the digest was verified when
/// [`install`] published the binary, which it flushed before the marker.
pub fn cached_by_size(tool_dir: &Path, asset: &Asset, os: &str) -> Option<PathBuf> {
    let version_dir = tool_dir.join(VERSION);
    if !version_dir.join(MARKER).is_file() {
        return None;
    }
    let binary = version_dir.join(binary_file_name(os));
    std::fs::metadata(&binary)
        .is_ok_and(|meta| meta.is_file() && meta.len() == asset.size)
        .then_some(binary)
}

fn binary_verified(binary: &Path, asset: &Asset) -> bool {
    std::fs::read(binary).is_ok_and(|bytes| verify(&bytes, asset).is_ok())
}

/// Installs whatever is missing and returns the binary path. An existing
/// binary is reused only after its digest is verified. A failed browser
/// install keeps the verified binary, so a retry only reruns that step.
pub fn install(
    tool_dir: &Path,
    asset: &Asset,
    os: &str,
    on_stage: &mut dyn FnMut(InstallStage),
    fetch: &dyn Fetch,
    run: &dyn Run,
) -> Result<PathBuf, InstallError> {
    let version_dir = tool_dir.join(VERSION);
    let binary = version_dir.join(binary_file_name(os));
    let marker = version_dir.join(MARKER);
    let reusable = binary_verified(&binary, asset);
    if reusable && marker.is_file() {
        return Ok(binary);
    }

    on_stage(InstallStage::Binary);
    if !reusable {
        let bytes = fetch.get(&download_url(asset))?;
        verify(&bytes, asset)?;
        std::fs::create_dir_all(&version_dir)?;
        publish(&version_dir, &binary, &bytes, Publish::Binary)?;
    }

    on_stage(InstallStage::Browser);
    run.run_install(&binary)?;

    publish(&version_dir, &marker, &[], Publish::Marker)?;
    Ok(binary)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Publish {
    Binary,
    Marker,
}

/// Writes `bytes` beside `target` and renames over it. The binary is flushed
/// first: [`cached_by_size`] trusts any marked binary of the right size.
/// `persist` replaces an existing file on Windows too (MOVEFILE_REPLACE_EXISTING).
fn publish(dir: &Path, target: &Path, bytes: &[u8], kind: Publish) -> std::io::Result<()> {
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(bytes)?;
    tmp.flush()?;
    if kind == Publish::Binary {
        make_executable(tmp.as_file())?;
        daruda_core::path::sync_file_data(tmp.as_file())?;
    }
    tmp.persist(target).map_err(|e| e.error)?;
    Ok(())
}

#[cfg(unix)]
fn make_executable(file: &std::fs::File) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    file.set_permissions(std::fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn make_executable(_: &std::fs::File) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests;
