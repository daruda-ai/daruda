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
    Request(#[source] Box<ureq::Error>),
    #[error("agent-browser download failed: {0}")]
    Read(#[source] std::io::Error),
    #[error("agent-browser download exceeds {max} bytes")]
    TooLarge { max: u64 },
    #[error(transparent)]
    Verify(#[from] BrowserToolError),
    #[error("agent-browser install I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("agent-browser browser install did not finish: {0}")]
    BrowserRun(#[source] CaptureError),
    #[error("agent-browser browser install failed: {status}: {stderr_tail}")]
    BrowserExit {
        status: std::process::ExitStatus,
        stderr_tail: String,
    },
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
            .map_err(|e| InstallError::Request(Box::new(e)))?;
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
        .map_err(InstallError::Read)?;
    if bytes.len() as u64 > max {
        return Err(InstallError::TooLarge { max });
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
    let output = result.map_err(InstallError::BrowserRun)?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(InstallError::BrowserExit {
        status: output.status,
        stderr_tail: tail(stderr.trim(), STDERR_TAIL_CHARS).to_owned(),
    })
}

fn tail(text: &str, max_chars: usize) -> &str {
    let skip = text.chars().count().saturating_sub(max_chars);
    text.char_indices()
        .nth(skip)
        .map_or("", |(index, _)| &text[index..])
}

/// One pinned release installed under a profile's tool directory:
/// `<tool_dir>/<VERSION>/<binary>` plus the `.complete` marker.
#[derive(Debug, Clone)]
pub struct ToolInstall {
    version_dir: PathBuf,
    asset: Asset,
    /// A `std::env::consts::OS` value; it names the binary.
    os: &'static str,
}

impl ToolInstall {
    pub fn new(tool_dir: &Path, asset: Asset, os: &'static str) -> Self {
        Self {
            version_dir: tool_dir.join(VERSION),
            asset,
            os,
        }
    }

    pub fn binary(&self) -> PathBuf {
        self.version_dir.join(binary_file_name(self.os))
    }

    fn marker(&self) -> PathBuf {
        self.version_dir.join(MARKER)
    }

    /// The installed binary, when the marker exists and the binary has the
    /// pinned size. Cheap enough for the UI thread: the digest was verified
    /// when [`Self::install`] published the binary, flushed before the marker.
    /// Trust boundary: a same-size file swapped in by someone with the
    /// user's write access passes; [`Self::install`] re-hashes before reuse.
    pub fn cached(&self) -> Option<PathBuf> {
        if !self.marker().is_file() {
            return None;
        }
        let binary = self.binary();
        std::fs::metadata(&binary)
            .is_ok_and(|meta| meta.is_file() && meta.len() == self.asset.size)
            .then_some(binary)
    }

    /// Installs whatever is missing and returns the binary path. An existing
    /// binary is reused only after its digest is verified. A failed browser
    /// install keeps the verified binary, so a retry only reruns that step.
    pub fn install(
        &self,
        on_stage: &mut dyn FnMut(InstallStage),
        fetch: &dyn Fetch,
        run: &dyn Run,
    ) -> Result<PathBuf, InstallError> {
        let binary = self.binary();
        let marker = self.marker();
        let reusable = self.binary_verified(&binary);
        if reusable && marker.is_file() {
            return Ok(binary);
        }

        on_stage(InstallStage::Binary);
        if !reusable {
            let bytes = fetch.get(&download_url(&self.asset))?;
            verify(&bytes, &self.asset)?;
            std::fs::create_dir_all(&self.version_dir)?;
            publish_binary(&self.version_dir, &binary, &bytes)?;
        }

        on_stage(InstallStage::Browser);
        run.run_install(&binary)?;

        write_marker(&self.version_dir, &marker)?;
        Ok(binary)
    }

    fn binary_verified(&self, binary: &Path) -> bool {
        std::fs::read(binary).is_ok_and(|bytes| verify(&bytes, &self.asset).is_ok())
    }
}

/// Writes `bytes` beside `target`, flushes it and renames over it:
/// [`ToolInstall::cached`] trusts any marked binary of the right size.
/// `persist` replaces an existing file on Windows too (MOVEFILE_REPLACE_EXISTING).
fn publish_binary(dir: &Path, target: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(bytes)?;
    tmp.flush()?;
    make_executable(tmp.as_file())?;
    daruda_core::path::sync_file_data(tmp.as_file())?;
    tmp.persist(target).map_err(|e| e.error)?;
    Ok(())
}

/// Creates the empty marker through a rename, so it appears whole.
fn write_marker(dir: &Path, marker: &Path) -> std::io::Result<()> {
    tempfile::NamedTempFile::new_in(dir)?
        .persist(marker)
        .map_err(|e| e.error)?;
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
