//! What can go wrong running `gh`, and what each failure means for polling.

use std::time::Duration;

#[derive(Debug)]
pub enum GhError {
    /// `gh` is not installed or not on `PATH`.
    NotFound,
    Spawn(std::io::Error),
    /// `gh` ran and exited non-zero; `stderr` is what it said.
    Exit {
        code: Option<i32>,
        stderr: String,
    },
    Timeout(Duration),
    /// The output exceeded its byte limit.
    Overflow {
        limit: usize,
    },
    /// The output was not what this crate expects from `gh`.
    Parse(String),
}

impl std::fmt::Display for GhError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "gh is not installed or not on PATH"),
            Self::Spawn(e) => write!(f, "spawn gh: {e}"),
            Self::Exit {
                code: Some(code),
                stderr,
            } => write!(f, "gh exited {code}: {}", stderr.trim()),
            Self::Exit { code: None, stderr } => {
                write!(f, "gh terminated by signal: {}", stderr.trim())
            }
            Self::Timeout(d) => write!(f, "gh timed out after {}s", d.as_secs()),
            Self::Overflow { limit } => write!(f, "gh output exceeded {limit} bytes"),
            Self::Parse(message) => write!(f, "unexpected gh output: {message}"),
        }
    }
}

impl std::error::Error for GhError {}

/// How a failed request should change what the poller does next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// `gh` is gone; ask the user to install it.
    Missing,
    /// `gh` has no usable login for the host; ask the user to log in.
    LoggedOut,
    /// GitHub refused for rate limiting; back off longer.
    RateLimited,
    /// Anything else, network trouble included; back off and retry.
    Transient,
}

impl GhError {
    pub fn kind(&self) -> FailureKind {
        match self {
            Self::NotFound => FailureKind::Missing,
            // `gh` exits 4 when a command needs a login it does not have.
            Self::Exit { code: Some(4), .. } => FailureKind::LoggedOut,
            // ASSUMPTION: not measured — a revoked or expired token reaches
            // `gh api` as GitHub's HTTP 401, which gh echoes on stderr.
            Self::Exit { stderr, .. } if stderr.contains("HTTP 401") => FailureKind::LoggedOut,
            // ASSUMPTION: not measured — gh relays GitHub's "API rate limit
            // exceeded" message on stderr; matched loosely in case it varies.
            Self::Exit { stderr, .. } if stderr.to_ascii_lowercase().contains("rate limit") => {
                FailureKind::RateLimited
            }
            _ => FailureKind::Transient,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_4_means_logged_out() {
        let error = GhError::Exit {
            code: Some(4),
            stderr: "To get started with GitHub CLI, please run:  gh auth login".into(),
        };
        assert_eq!(error.kind(), FailureKind::LoggedOut);
    }
}
