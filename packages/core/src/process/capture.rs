//! Run a child to completion with its output captured, bounded in time and
//! size, and torn down as a tree when either bound is crossed.

use std::convert::Infallible;
use std::io::Read;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
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
            Self::Timeout(d) => write!(f, "timed out after {:.1}s", d.as_secs_f64()),
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
pub fn run_captured(command: Command, limits: CaptureLimits) -> Result<Captured, CaptureError> {
    uncancelled(run(command, None, limits, Uncancellable))
}

/// [`run_captured`], with `input` written to the child's stdin and then
/// closed: for text too long to pass as an argument, which every OS caps
/// (a whole Windows command line at 32,767 UTF-16 units).
pub fn run_captured_with_input(
    command: Command,
    input: Vec<u8>,
    limits: CaptureLimits,
) -> Result<Captured, CaptureError> {
    uncancelled(run(command, Some(input), limits, Uncancellable))
}

/// The caller raised its flag and the child was killed with its tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

/// [`run_captured_with_input`] that also ends, killing the child's tree,
/// once `cancelled` is raised: for a run nobody waits on any longer, which
/// would otherwise hold its process until the deadline.
pub fn run_captured_until_cancelled(
    command: Command,
    input: Vec<u8>,
    limits: CaptureLimits,
    cancelled: &AtomicBool,
) -> Result<Result<Captured, Cancelled>, CaptureError> {
    run(command, Some(input), limits, cancelled)
}

/// Whether a run is asked to end early. The answer's type says whether it
/// can be: a run nothing cancels has no cancelled outcome to handle.
trait Cancel {
    type Stopped;
    fn stopped(&self) -> Option<Self::Stopped>;
}

struct Uncancellable;

impl Cancel for Uncancellable {
    type Stopped = Infallible;
    fn stopped(&self) -> Option<Infallible> {
        None
    }
}

impl Cancel for &AtomicBool {
    type Stopped = Cancelled;
    fn stopped(&self) -> Option<Cancelled> {
        self.load(Ordering::Relaxed).then_some(Cancelled)
    }
}

fn uncancelled(
    outcome: Result<Result<Captured, Infallible>, CaptureError>,
) -> Result<Captured, CaptureError> {
    outcome.map(|ran| match ran {
        Ok(captured) => captured,
        Err(never) => match never {},
    })
}

fn run<C: Cancel>(
    mut command: Command,
    input: Option<Vec<u8>>,
    limits: CaptureLimits,
    cancel: C,
) -> Result<Result<Captured, C::Stopped>, CaptureError> {
    if input.is_some() {
        command.stdin(Stdio::piped());
    }
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    lead_own_group(&mut command);
    // Stopped before it began: nothing is started only to be ended.
    if let Some(stopped) = cancel.stopped() {
        return Ok(Err(stopped));
    }
    let mut child = command.spawn().map_err(CaptureError::Spawn)?;
    let group = match Group::try_adopt(child.id()) {
        Ok(group) => group,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(CaptureError::Spawn(error));
        }
    };

    // Written from its own thread, so a child that reads slowly, or not
    // at all, never stalls the drains or the deadline; closing stdin after
    // is the end of input the child waits for.
    let input_thread = input.and_then(|input| {
        let mut stdin = child.stdin.take()?;
        Some(thread::spawn(move || {
            use std::io::Write as _;
            let _ = stdin.write_all(&input);
        }))
    });
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
                if let Some(stopped) = cancel.stopped() {
                    // As at the deadline: the tree ends, the drains detach.
                    let termination = terminate(&mut child, &group);
                    drop(stdout_thread);
                    drop(stderr_thread);
                    termination?;
                    return Ok(Err(stopped));
                }
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
    // The child exited, so a write still blocked would have failed on the
    // closed pipe: nothing is left to wait on.
    drop(input_thread);

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
    Ok(Ok(out))
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
