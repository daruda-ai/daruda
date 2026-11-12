//! Run a child to completion with its output captured, bounded in time and
//! size, and torn down as a tree when either bound is crossed.

use std::io::Read;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use super::{Group, lead_own_group};

/// Granularity of the exit poll: short enough that a quick command feels
/// synchronous, long enough that polling stays well under 1% of a CPU.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// How long the pipes may stay open after the root exits before the drain
/// gives up: a helper that inherited them can keep them open indefinitely.
const DRAIN_GRACE: Duration = Duration::from_secs(2);

/// The bounds [`run_captured`] enforces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureLimits {
    pub timeout: Duration,
    pub stdout: usize,
    pub stderr: usize,
}

/// A child that ran to completion within its [`CaptureLimits`].
#[derive(Debug)]
pub struct Captured {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Why [`run_captured`] has no [`Captured`] to return.
#[derive(Debug)]
pub enum CaptureError {
    /// Spawning, adopting, polling or tearing down the child failed.
    Spawn(std::io::Error),
    /// The child outlived the deadline and was killed with its tree.
    Timeout(Duration),
    /// One stream exceeded its byte limit and the child was killed.
    Overflow { limit: usize },
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn(e) => write!(f, "spawn: {e}"),
            Self::Timeout(d) => write!(f, "timed out after {}s", d.as_secs()),
            Self::Overflow { limit } => write!(f, "output exceeded {limit} bytes"),
        }
    }
}

impl std::error::Error for CaptureError {}

/// Run `command` with piped output until it exits. Stdin is left as the
/// caller configured it.
///
/// The child leads its own process group, so a timeout or an overflow ends
/// the helpers it forked (ssh, credential helpers) as well as the child.
/// Both streams drain concurrently, so a full pipe never stalls the child.
pub fn run_captured(mut command: Command, limits: CaptureLimits) -> Result<Captured, CaptureError> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    lead_own_group(&mut command);
    let mut child = command.spawn().map_err(CaptureError::Spawn)?;
    let group = match Group::try_adopt(child.id()) {
        Ok(group) => group,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(CaptureError::Spawn(error));
        }
    };

    // Invariant: both streams were piped above; std guarantees Some after spawn.
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let (output_tx, output_rx) = mpsc::channel();
    let stderr_tx = output_tx.clone();
    let stdout_thread = thread::spawn(move || {
        let _ = output_tx.send((Stream::Stdout, capture(stdout, limits.stdout)));
    });
    let stderr_thread = thread::spawn(move || {
        let _ = stderr_tx.send((Stream::Stderr, capture(stderr, limits.stderr)));
    });

    let started = Instant::now();
    let mut captured = Vec::with_capacity(2);
    let status = loop {
        while let Ok((stream, result)) = output_rx.try_recv() {
            match result {
                Ok(bytes) => captured.push((stream, bytes)),
                Err(error) => {
                    terminate(&mut child, &group)?;
                    return Err(error);
                }
            }
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if started.elapsed() >= limits.timeout {
                    // The whole tree, so no helper is left holding the pipes
                    // the drain threads read. Unreaped until the wait inside,
                    // so the pid is still ours to name.
                    let termination = terminate(&mut child, &group);
                    // Detached rather than joined: the caller gets `Timeout`
                    // now, not after the OS has finished closing the pipes.
                    drop(stdout_thread);
                    drop(stderr_thread);
                    termination?;
                    return Err(CaptureError::Timeout(limits.timeout));
                }
                thread::sleep(POLL_INTERVAL);
            }
            Err(error) => {
                terminate(&mut child, &group)?;
                return Err(CaptureError::Spawn(error));
            }
        }
    };

    // Root exit does not guarantee EOF: helpers can inherit either pipe.
    let drain_deadline = (started + limits.timeout).min(Instant::now() + DRAIN_GRACE);
    for _ in captured.len()..2 {
        let (stream, result) = output_rx
            .recv_timeout(drain_deadline.saturating_duration_since(Instant::now()))
            .map_err(|_| CaptureError::Timeout(limits.timeout))?;
        captured.push((stream, result?));
    }
    let _ = stdout_thread.join();
    let _ = stderr_thread.join();

    let mut out = Captured {
        status,
        stdout: Vec::new(),
        stderr: Vec::new(),
    };
    for (stream, bytes) in captured {
        match stream {
            Stream::Stdout => out.stdout = bytes,
            Stream::Stderr => out.stderr = bytes,
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, Copy)]
enum Stream {
    Stdout,
    Stderr,
}

fn terminate(child: &mut std::process::Child, group: &Group) -> Result<(), CaptureError> {
    group
        .try_terminate_child(child)
        .map_err(CaptureError::Spawn)
}

fn capture(mut reader: impl Read, limit: usize) -> Result<Vec<u8>, CaptureError> {
    let mut bytes = Vec::new();
    let mut chunk = [0; 8192];
    loop {
        let count = reader.read(&mut chunk).map_err(CaptureError::Spawn)?;
        if count == 0 {
            return Ok(bytes);
        }
        if count > limit.saturating_sub(bytes.len()) {
            return Err(CaptureError::Overflow { limit });
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_stops_at_the_limit() {
        let input: &[u8] = &[b'x'; 20];
        assert!(matches!(
            capture(input, 10),
            Err(CaptureError::Overflow { limit: 10 })
        ));
        assert_eq!(capture(input, 20).unwrap().len(), 20);
    }
}
