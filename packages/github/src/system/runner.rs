//! Running `gh`: argv only, no prompt, output captured within limits.

use std::ffi::OsStr;
use std::process::Stdio;
use std::time::Duration;

use daruda_core::process::{
    CaptureError, CaptureLimits, Captured, command_on_path, run_captured, run_captured_with_input,
};

use super::SystemGh;
use crate::GhError;

const LIMITS: CaptureLimits = CaptureLimits {
    timeout: Duration::from_secs(30),
    stdout: 16 * 1024 * 1024,
    stderr: 1024 * 1024,
};

impl SystemGh {
    pub(super) fn run<I, S>(&self, args: I) -> Result<Captured, GhError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.run_in(None, args)
    }

    pub(super) fn run_in<I, S>(
        &self,
        cwd: Option<&std::path::Path>,
        args: I,
    ) -> Result<Captured, GhError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.run_fed(cwd, args, None)
    }

    /// `gh` with `input` on its stdin: for text an argument cannot carry,
    /// as every OS caps the command line.
    pub(super) fn stdout_fed<I, S>(&self, args: I, input: Vec<u8>) -> Result<String, GhError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        answer(self.run_fed(None, args, Some(input))?)
    }

    pub(super) fn run_fed<I, S>(
        &self,
        cwd: Option<&std::path::Path>,
        args: I,
        input: Option<Vec<u8>>,
    ) -> Result<Captured, GhError>
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
        let ran = match input {
            Some(input) => run_captured_with_input(command, input, LIMITS),
            None => run_captured(command, LIMITS),
        };
        match ran {
            Ok(out) => Ok(out),
            Err(CaptureError::Spawn(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(GhError::NotFound)
            }
            Err(CaptureError::Spawn(e)) => Err(GhError::Spawn(e)),
            Err(CaptureError::Timeout(d)) => Err(GhError::Timeout(d)),
            Err(CaptureError::Overflow { limit }) => Err(GhError::Overflow { limit }),
        }
    }

    pub(super) fn stdout<I, S>(&self, args: I) -> Result<String, GhError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.stdout_in(None, args)
    }

    pub(super) fn stdout_in<I, S>(
        &self,
        cwd: Option<&std::path::Path>,
        args: I,
    ) -> Result<String, GhError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        answer(self.run_in(cwd, args)?)
    }
}

/// What `gh` printed, if it succeeded.
pub(super) fn answer(out: Captured) -> Result<String, GhError> {
    if !out.status.success() {
        return Err(exit(&out));
    }
    String::from_utf8(out.stdout).map_err(|_| GhError::Parse("output is not UTF-8".into()))
}

/// How `gh` failed, from its exit and its error output.
pub(super) fn exit(out: &Captured) -> GhError {
    GhError::Exit {
        code: out.status.code(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}
