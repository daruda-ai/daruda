//! Each tab's count, from the same per-repository status its group reads.
//! What is known is counted; a repository that could not be read, a capped
//! search, or a stale answer makes the count a lower bound (`N+`).

use crate::surface::strings::pull_requests as s;
use crate::workspace::github_ops::pr_list::PrListCache;
use daruda_github::availability::GhAvailability;
use daruda_github::{PrAudience, PrSearchState, RepoId};
use std::collections::BTreeSet;

use super::list::{RepoStatus, repo_status};

/// What one tab's count comes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tally {
    /// A repository is still waiting on its first answer.
    Pending,
    /// No repository could be read at all.
    Unavailable,
    Known {
        count: usize,
        lower_bound: bool,
    },
}

pub(super) fn labels(
    list: &PrListCache,
    state: PrSearchState,
    repos: &BTreeSet<RepoId>,
    availability: impl Fn(&str) -> GhAvailability,
) -> [String; 3] {
    super::list::FILTERS.map(
        |filter| match tally(list, repos, filter, state, &availability) {
            Tally::Pending => s::count_pending(),
            Tally::Unavailable => s::count_unavailable(),
            Tally::Known {
                count,
                lower_bound: true,
            } => s::count_capped(count),
            Tally::Known {
                count,
                lower_bound: false,
            } => count.to_string(),
        },
    )
}

fn tally(
    list: &PrListCache,
    repos: &BTreeSet<RepoId>,
    filter: PrAudience,
    state: PrSearchState,
    availability: &impl Fn(&str) -> GhAvailability,
) -> Tally {
    let (mut count, mut known, mut lower_bound) = (0, false, false);
    for repo in repos {
        let host = repo.host();
        match repo_status(repo, availability(host), list.host(filter, state, host)) {
            RepoStatus::Loading => return Tally::Pending,
            RepoStatus::Blocked(_) | RepoStatus::Listed { search: None, .. } => {
                lower_bound = true;
            }
            RepoStatus::Listed {
                search: Some(search),
                trouble,
            } => {
                known = true;
                lower_bound |= search.truncated || trouble.is_some();
                count += search
                    .items
                    .iter()
                    .filter(|item| item.repo == *repo)
                    .count();
            }
        }
    }
    if lower_bound && !known {
        Tally::Unavailable
    } else {
        Tally::Known { count, lower_bound }
    }
}

#[cfg(test)]
mod tests;
