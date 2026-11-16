//! Counts use complete host coverage; a capped search is a lower bound.

use crate::workspace::github_ops::pr_list::{HostList, PrListState};
use daruda_github::{PrFilter, RepoId};
use std::collections::BTreeSet;

pub(super) fn labels(list: &PrListState, repos: &BTreeSet<RepoId>) -> [String; 3] {
    super::list::FILTERS.map(|filter| {
        count(list, repos, filter).map_or_else(
            crate::surface::strings::pull_requests::count_pending,
            |(count, capped)| {
                if capped {
                    crate::surface::strings::pull_requests::count_capped(count)
                } else {
                    count.to_string()
                }
            },
        )
    })
}

fn count(list: &PrListState, repos: &BTreeSet<RepoId>, filter: PrFilter) -> Option<(usize, bool)> {
    let mut items = BTreeSet::new();
    let mut capped = false;
    for repo in repos {
        let HostList::Ready(answer) = list.host_for(filter, repo.host()) else {
            return None;
        };
        if !answer.covered.contains(repo) {
            return None;
        }
        capped |= answer.search.truncated;
        items.extend(
            answer
                .search
                .items
                .iter()
                .filter(|item| &item.repo == repo)
                .map(|item| (item.repo.clone(), item.pr.number)),
        );
    }
    Some((items.len(), capped))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_coverage_is_unknown_and_an_empty_scope_is_zero() {
        let list = PrListState::Hidden;
        assert_eq!(
            count(&list, &BTreeSet::new(), PrFilter::Mine),
            Some((0, false))
        );
        let repos = [RepoId::new("github.com", "o", "r")].into();
        assert_eq!(count(&list, &repos, PrFilter::Mine), None);
    }
    #[test]
    fn capped_and_partial_answers_never_look_like_exact_complete_counts() {
        use crate::github::PrListAnswer;
        use daruda_github::PrSearch;
        use std::{collections::HashMap, sync::Arc, time::Instant};
        let repo = RepoId::new("github.com", "o", "r");
        let answer = PrListAnswer {
            search: Arc::new(PrSearch {
                items: Vec::new(),
                truncated: true,
            }),
            covered: Arc::new([repo.clone()].into()),
            at: Instant::now(),
        };
        let state = PrListState::Shown {
            filter: PrFilter::Mine,
            hosts: HashMap::from([(
                (PrFilter::Mine, "github.com".into()),
                HostList::Ready(answer),
            )]),
            _aging: gpui::Task::ready(()),
        };
        let repos = [repo.clone()].into();
        assert_eq!(count(&state, &repos, PrFilter::Mine), Some((0, true)));
        assert_eq!(labels(&state, &repos)[0], "0+");
        assert_eq!(count(&state, &repos, PrFilter::AllOpen), None);
        let larger = [repo, RepoId::new("github.com", "o", "other")].into();
        assert_eq!(count(&state, &larger, PrFilter::Mine), None);
    }
}
