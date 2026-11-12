//! `[github]` — pull-request status for lanes, read through the `gh` CLI.

use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct GithubConfig {
    /// Look up each lane's pull request and its checks.
    pub enabled: bool,
    /// How often the repository of the focused lane is re-read. Clamped to
    /// [`Self::MIN_POLL_SECS`]: every read spends GitHub API quota.
    pub poll_secs: u64,
}

const DEFAULT_POLL_SECS: u64 = 30;

impl Default for GithubConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_secs: DEFAULT_POLL_SECS,
        }
    }
}

impl GithubConfig {
    pub const MIN_POLL_SECS: u64 = 10;

    /// The focused repository's cadence, clamped to [`Self::MIN_POLL_SECS`].
    pub fn interval(&self) -> Duration {
        Duration::from_secs(self.poll_secs.max(Self::MIN_POLL_SECS))
    }
}
