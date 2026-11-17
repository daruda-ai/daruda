//! Scripted pages use the production parser, including cursor and coverage
//! rules. The full request keys each answer, so scopes cannot share pages.

use super::{FakeGh, GhCall, PrSearchPage, PrSearchRequest};
use crate::query::{newest_first, parse_search_page};
use crate::{GhError, PrAudience, PrFilter, PrListItem, PrSearchState, PrState};

impl FakeGh {
    /// Script the GraphQL JSON that this exact request answers. Parsing is
    /// deferred until asked, so malformed and partial replies can be tested.
    pub fn set_search_page(&self, request: &PrSearchRequest, json: &str) {
        self.lock()
            .search_pages
            .insert(request.clone(), json.into());
    }

    pub(super) fn search_page(&self, request: &PrSearchRequest) -> Result<PrSearchPage, GhError> {
        let mut state = self.lock();
        state.record(GhCall::SearchPrsPage {
            request: Box::new(request.clone()),
        })?;
        if request.repos().is_empty() {
            return request.finish_page(Vec::new(), 0, 0, false, None);
        }
        match state.search_pages.get(request) {
            Some(json) => parse_search_page(json, request),
            None => state.listed_page(request),
        }
    }
}

/// Opaque to callers, as GitHub's are; only this fake reads it back.
const CURSOR_PREFIX: &str = "fake-offset:";

impl super::FakeState {
    /// Without a scripted reply, the lists `set_pr_list` scripted answer:
    /// the audience's list, narrowed to the request's repositories, state
    /// and title words, newest first, one page at a time. `set_search_truncated`
    /// reports matches the list left out.
    fn listed_page(&self, request: &PrSearchRequest) -> Result<PrSearchPage, GhError> {
        let criteria = request.criteria();
        let filter = match criteria.audience {
            PrAudience::Mine => PrFilter::Mine,
            PrAudience::ReviewRequested => PrFilter::ReviewRequested,
            PrAudience::All => PrFilter::AllOpen,
        };
        let mut items: Vec<PrListItem> = self
            .lists
            .get(&filter)
            .into_iter()
            .flatten()
            .filter(|item| request.repos().contains(&item.repo))
            .filter(|item| match criteria.state {
                PrSearchState::Open => item.pr.state.is_active(),
                PrSearchState::Merged => item.pr.state == PrState::Merged,
                PrSearchState::Closed => item.pr.state == PrState::Closed,
                PrSearchState::All => true,
            })
            // Words match the title, as GitHub's matches it among others;
            // facets are not applied.
            .filter(|item| {
                let title = item.pr.title.to_lowercase();
                criteria
                    .terms
                    .iter()
                    .all(|term| title.contains(&term.to_lowercase()))
            })
            .cloned()
            .collect();
        newest_first(&mut items);
        let offset = match request.after() {
            None => 0,
            Some(cursor) => cursor
                .strip_prefix(CURSOR_PREFIX)
                .and_then(|n| n.parse::<usize>().ok())
                .ok_or_else(|| GhError::Parse(format!("unknown fake cursor {cursor}")))?,
        };
        let total = items.len();
        let end = (offset + usize::from(request.page_size())).min(total);
        let page: Vec<PrListItem> = items.get(offset..end).unwrap_or_default().to_vec();
        let omitted = u32::from(self.truncated.get(&filter).copied().unwrap_or(false));
        let count = page.len() as u32;
        request.finish_page(
            page,
            total as u32 + omitted,
            count,
            end < total,
            Some(format!("{CURSOR_PREFIX}{end}")),
        )
    }
}

#[cfg(test)]
mod tests;
