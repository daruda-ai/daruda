//! How a lane lookup is split into requests and its answers read.

use daruda_core::process::Captured;

use crate::GhError;
use crate::query::{RepoPrs, only_numbers_not_found, parse_prs_for_branches};

/// Branches, and separately numbers, per request; the query grows with each.
const BRANCHES_PER_REQUEST: usize = 50;

/// A lane lookup's answer from what `gh` printed. One that failed only
/// because a number names no pull request still carries the rest; any other
/// failure is the request's.
pub(super) fn read_lane_answer(
    out: &Captured,
    branches: &[String],
    numbers: &[u64],
) -> Result<RepoPrs, GhError> {
    let json = std::str::from_utf8(&out.stdout).unwrap_or_default();
    if out.status.success() {
        return parse_prs_for_branches(json, branches, numbers);
    }
    let failed = super::runner::exit(out);
    if !only_numbers_not_found(json) {
        return Err(failed);
    }
    parse_prs_for_branches(json, branches, numbers).map_err(|_| failed)
}

/// The branches and numbers of each request a lane lookup sends: as many
/// requests as the longer list needs, the shorter one riding along.
pub(super) fn lane_requests<'a>(
    branches: &'a [String],
    numbers: &'a [u64],
) -> Vec<(&'a [String], &'a [u64])> {
    let branches: Vec<&[String]> = branches.chunks(BRANCHES_PER_REQUEST).collect();
    let numbers: Vec<&[u64]> = numbers.chunks(BRANCHES_PER_REQUEST).collect();
    (0..branches.len().max(numbers.len()))
        .map(|i| {
            (
                branches.get(i).copied().unwrap_or_default(),
                numbers.get(i).copied().unwrap_or_default(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests;
