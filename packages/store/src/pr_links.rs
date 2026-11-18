//! How the user tied worktrees to pull requests, kept across restarts: a
//! pull request chosen for a worktree's branch, automatic (branch-name)
//! links the user took away, and each worktree's past links. Keyed by the
//! worktree's path and branch, so a branch switched to later starts clean.
//! Profile-scoped through the data directory; daruda is the sole writer.
//!
//! ```text
//! <data dir>/pr-links.json
//! ```

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::observability::error_report::{ErrorReport, ErrorSeverity};
use crate::observability::log_writer::LogWriter;
use crate::observability::system_info::redact_home;
use crate::persistence::{LoadOutcome, load_json_file, save_json_atomic};

/// The file's layout version: a newer one is not loaded, so fields this
/// build does not know are never written away.
pub const SCHEMA_VERSION: u32 = 1;

/// A pull request, by what never changes about it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PrRef {
    pub host: String,
    pub owner: String,
    pub name: String,
    pub number: u64,
}

/// Why a worktree was tied to a pull request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LinkSource {
    /// Its branch heads the pull request, found by name.
    Branch,
    /// The user chose the pull request for it.
    Chosen,
    /// It was checked out to review a fork's pull request.
    Review,
}

/// One time a worktree was tied to a pull request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkEvent {
    pub pr: PrRef,
    pub source: LinkSource,
    /// Unix seconds.
    pub linked_at: i64,
    /// When the user took it away; `None` while it holds or until known.
    #[serde(default)]
    pub unlinked_at: Option<i64>,
}

/// What the user decided for one worktree on one branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeLinks {
    pub worktree: PathBuf,
    pub branch: String,
    /// The pull request chosen for it, over any its branch name finds.
    #[serde(default)]
    pub chosen: Option<PrRef>,
    /// Pull requests its branch name finds that it is not to be tied to.
    #[serde(default)]
    pub suppressed: Vec<PrRef>,
    /// Past links, oldest first.
    #[serde(default)]
    pub history: Vec<LinkEvent>,
}

impl WorktreeLinks {
    fn is_empty(&self) -> bool {
        self.chosen.is_none() && self.suppressed.is_empty() && self.history.is_empty()
    }
}

/// Past links kept per worktree and branch; older ones are dropped.
const HISTORY_MAX: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrLinks {
    pub schema_version: u32,
    #[serde(default)]
    pub worktrees: Vec<WorktreeLinks>,
}

impl Default for PrLinks {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            worktrees: Vec::new(),
        }
    }
}

impl PrLinks {
    pub fn of(&self, worktree: &Path, branch: &str) -> Option<&WorktreeLinks> {
        self.worktrees
            .iter()
            .find(|held| held.worktree == worktree && held.branch == branch)
    }

    fn entry(&mut self, worktree: &Path, branch: &str) -> &mut WorktreeLinks {
        let at = match self
            .worktrees
            .iter()
            .position(|held| held.worktree == worktree && held.branch == branch)
        {
            Some(at) => at,
            None => {
                self.worktrees.push(WorktreeLinks {
                    worktree: worktree.to_owned(),
                    branch: branch.to_owned(),
                    chosen: None,
                    suppressed: Vec::new(),
                    history: Vec::new(),
                });
                self.worktrees.len() - 1
            }
        };
        &mut self.worktrees[at]
    }

    /// Tie the worktree's branch to `pr`, over what its name finds.
    pub fn choose(&mut self, worktree: &Path, branch: &str, pr: PrRef, now: i64) {
        let links = self.entry(worktree, branch);
        links.suppressed.retain(|held| *held != pr);
        links.chosen = Some(pr.clone());
        push_history(links, pr, LinkSource::Chosen, now);
    }

    /// Take `pr` away from the worktree's branch: a chosen one is dropped,
    /// and with `named` (its name would find it) it is not shown again until
    /// restored. History keeps it.
    pub fn unlink(
        &mut self,
        worktree: &Path,
        branch: &str,
        pr: PrRef,
        source: LinkSource,
        named: bool,
        now: i64,
    ) {
        let links = self.entry(worktree, branch);
        if links.chosen.as_ref() == Some(&pr) {
            links.chosen = None;
        }
        // The branch name would find it: kept away until restored.
        if named && !links.suppressed.contains(&pr) {
            links.suppressed.push(pr.clone());
        }
        match links
            .history
            .iter_mut()
            .rev()
            .find(|event| event.pr == pr && event.unlinked_at.is_none())
        {
            Some(event) => event.unlinked_at = Some(now),
            None => {
                push_history(links, pr, source, now);
                if let Some(event) = links.history.last_mut() {
                    event.unlinked_at = Some(now);
                }
            }
        }
    }

    /// Let the worktree's branch name find its pull requests again.
    pub fn restore(&mut self, worktree: &Path, branch: &str) {
        self.entry(worktree, branch).suppressed.clear();
    }

    /// Record that `pr` is now the branch's, once per run of links.
    pub fn found(
        &mut self,
        worktree: &Path,
        branch: &str,
        pr: PrRef,
        source: LinkSource,
        now: i64,
    ) {
        let links = self.entry(worktree, branch);
        // The one it replaces is no longer the branch's.
        for event in links
            .history
            .iter_mut()
            .filter(|event| event.pr != pr && event.unlinked_at.is_none())
        {
            event.unlinked_at = Some(now);
        }
        let open = links
            .history
            .iter()
            .rev()
            .any(|event| event.pr == pr && event.unlinked_at.is_none());
        if !open {
            push_history(links, pr, source, now);
        }
    }

    /// Drop records with nothing left in them.
    pub fn prune(&mut self) {
        self.worktrees.retain(|held| !held.is_empty());
    }
}

fn push_history(links: &mut WorktreeLinks, pr: PrRef, source: LinkSource, now: i64) {
    links.history.push(LinkEvent {
        pr,
        source,
        linked_at: now,
        unlinked_at: None,
    });
    if links.history.len() > HISTORY_MAX {
        let extra = links.history.len() - HISTORY_MAX;
        links.history.drain(..extra);
    }
}

/// `pr-links.json` under `data_dir`.
pub fn pr_links_path_in(data_dir: &Path) -> PathBuf {
    data_dir.join("pr-links.json")
}

/// The links under `data_dir`: none when the file is missing. `None` for
/// a damaged file or one from a newer daruda, which must not be replaced
/// by an empty set.
pub fn load_pr_links_in(data_dir: &Path) -> Option<PrLinks> {
    let path = pr_links_path_in(data_dir);
    let links = match load_json_file::<PrLinks>("pr-links", &path) {
        LoadOutcome::Parsed(links) => links,
        LoadOutcome::Missing => return Some(PrLinks::default()),
        LoadOutcome::Corrupt => return None,
    };
    if links.schema_version > SCHEMA_VERSION {
        LogWriter::log(
            ErrorReport::new("pr-links.json from a newer daruda — refusing to load")
                .severity(ErrorSeverity::Warning)
                .at(file!(), line!())
                .with_context("path", redact_home(&path))
                .with_context("found", links.schema_version.to_string())
                .with_context("supported", SCHEMA_VERSION.to_string())
                .dedup("pr_links.schema.too_new")
                .build(),
        );
        return None;
    }
    let mut links = links;
    links.schema_version = SCHEMA_VERSION;
    Some(links)
}

/// Save atomically: a tempfile in the same directory, then renamed in.
pub fn save_pr_links_in(data_dir: &Path, links: &PrLinks) -> std::io::Result<()> {
    save_json_atomic(data_dir, &pr_links_path_in(data_dir), links)
}

/// The profile's data directory, where the links live.
pub fn pr_links_dir() -> PathBuf {
    crate::persistence::default_data_dir()
}

#[cfg(test)]
mod tests;
