//! A bounded provider search and its continuation. Each repository chunk
//! owns its cursor; a continuation retains the original scope and criteria.

use crate::{GhError, PrListItem, RepoId};

pub(crate) const SEARCH_PAGE_MAX: u8 = 50;
pub(crate) const SEARCH_REPOS_MAX: usize = 20;
const SEARCH_LIMIT: u32 = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PrAudience {
    #[default]
    Mine,
    ReviewRequested,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PrSearchState {
    /// Includes drafts.
    #[default]
    Open,
    Merged,
    /// Closed without merging.
    Closed,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PrReviewFilter {
    None,
    Required,
    Approved,
    ChangesRequested,
}

/// GitHub's `status:` search qualifier, not the application's check rollup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PrCommitStatus {
    Pending,
    Success,
    Failure,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PrRequestedReviewer {
    User(String),
    Team(String),
}

/// Supported provider qualifiers. Repository scope and lifecycle state are
/// separate from text, so text cannot widen the search to other repositories.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct PrSearchCriteria {
    pub audience: PrAudience,
    pub state: PrSearchState,
    /// Literal words or phrases, combined with AND. No raw search syntax.
    pub terms: Vec<String>,
    /// Requires an audience other than Mine; repeated author qualifiers can
    /// broaden provider results instead of intersecting with the audience.
    pub author: Option<String>,
    pub assignee: Option<String>,
    pub label: Option<String>,
    /// Requires an audience other than ReviewRequested.
    pub requested_reviewer: Option<PrRequestedReviewer>,
    pub review: Option<PrReviewFilter>,
    pub draft: Option<bool>,
    pub commit_status: Option<PrCommitStatus>,
    pub head: Option<String>,
    pub base: Option<String>,
}

/// One page from one host and at most 20 repositories. Split larger scopes
/// before requesting them; never reuse one chunk's continuation for another.
/// Account/generation and refresh-epoch isolation belong to the caller's cache.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PrSearchRequest {
    host: String,
    repos: Vec<RepoId>,
    criteria: PrSearchCriteria,
    page_size: u8,
    cursors: Vec<String>,
    scanned: u32,
    incomplete: bool,
}

impl PrSearchRequest {
    pub fn new(
        host: &str,
        repos: &[RepoId],
        criteria: PrSearchCriteria,
        page_size: u8,
    ) -> Result<Self, GhError> {
        let mut repos = repos.to_vec();
        repos.sort();
        repos.dedup();
        if !(1..=SEARCH_PAGE_MAX).contains(&page_size) {
            return Err(GhError::BadArgument(format!(
                "search page size must be 1..={SEARCH_PAGE_MAX}"
            )));
        }
        if repos.len() > SEARCH_REPOS_MAX {
            return Err(GhError::BadArgument(format!(
                "search chunk exceeds {SEARCH_REPOS_MAX} repositories"
            )));
        }
        for repo in &repos {
            if !repo.host().eq_ignore_ascii_case(host)
                || !repository_part(repo.owner())
                || !repository_part(repo.name())
            {
                return Err(GhError::BadArgument(format!(
                    "invalid search repository {} for host {host}",
                    repo.qualified()
                )));
            }
        }
        let request = Self {
            host: host.to_ascii_lowercase(),
            repos,
            criteria,
            page_size,
            cursors: Vec::new(),
            scanned: 0,
            incomplete: false,
        };
        crate::query::paged_search_string(&request)?;
        Ok(request)
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn repos(&self) -> &[RepoId] {
        &self.repos
    }

    pub fn criteria(&self) -> &PrSearchCriteria {
        &self.criteria
    }

    pub fn page_size(&self) -> u8 {
        self.page_size
    }

    pub fn after(&self) -> Option<&str> {
        self.cursors.last().map(String::as_str)
    }

    pub(crate) fn finish_page(
        &self,
        items: Vec<PrListItem>,
        total_count: u32,
        raw_count: u32,
        has_next: bool,
        end_cursor: Option<String>,
    ) -> Result<PrSearchPage, GhError> {
        if raw_count > u32::from(self.page_size) {
            return Err(GhError::Parse("search page exceeds requested size".into()));
        }
        let scanned = self.scanned + raw_count;
        let incomplete = self.incomplete || items.len() != raw_count as usize;
        let continuation = if has_next && scanned < SEARCH_LIMIT {
            let cursor = end_cursor.filter(|c| !c.is_empty()).ok_or_else(|| {
                GhError::Parse("search has another page without an end cursor".into())
            })?;
            if raw_count == 0 || self.cursors.contains(&cursor) {
                return Err(GhError::Parse("search cursor did not advance".into()));
            }
            let mut next = self.clone();
            next.cursors.push(cursor);
            next.scanned = scanned;
            next.incomplete = incomplete;
            PrSearchContinuation::More(Box::new(next))
        } else if total_count > SEARCH_LIMIT || has_next {
            PrSearchContinuation::SearchLimit
        } else if incomplete || scanned < total_count {
            PrSearchContinuation::Incomplete
        } else {
            PrSearchContinuation::Complete
        };
        Ok(PrSearchPage {
            items,
            total_count,
            continuation,
        })
    }
}

fn repository_part(part: &str) -> bool {
    !part.is_empty()
        && part
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrSearchPage {
    /// Provider order: most recently updated first. Pages are not a snapshot;
    /// consumers must deduplicate by repository and number when accumulating.
    pub items: Vec<PrListItem>,
    /// Matches visible to this account, including results beyond GitHub's
    /// search cap. Zero does not establish that a private resource is absent.
    pub total_count: u32,
    pub continuation: PrSearchContinuation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrSearchContinuation {
    More(Box<PrSearchRequest>),
    /// The provider exhausted the connection without detected omissions.
    Complete,
    /// The search cap prevents enumerating all matches; narrow the criteria.
    SearchLimit,
    /// Missing nodes or a short connection prevent claiming full coverage.
    Incomplete,
}

#[cfg(test)]
mod tests;
