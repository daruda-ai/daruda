//! Background-only metadata snapshots, including recoverable negative results.

use std::path::Path;
use std::time::SystemTime;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Version {
    File {
        len: u64,
        modified: Option<SystemTime>,
    },
    Read,
    NotFile,
}

impl Version {
    pub(super) fn read(path: &Path) -> Self {
        match std::fs::metadata(path) {
            Ok(metadata) if metadata.is_file() => Self::File {
                len: metadata.len(),
                modified: metadata.modified().ok(),
            },
            Ok(_) => Self::NotFile,
            Err(_) => Self::Read,
        }
    }
}

#[cfg(test)]
mod tests;
