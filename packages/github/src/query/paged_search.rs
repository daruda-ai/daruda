//! Cursor search transport and parsing; partial GraphQL answers are errors,
//! so a failed chunk cannot masquerade as a complete empty result.

use serde::Deserialize;

use super::search::{is_not_a_pull_request, parse_search_node, search_query};
use super::{ApiError, api_messages, parse_error};
use crate::{
    GhError, PrAudience, PrCommitStatus, PrRequestedReviewer, PrReviewFilter, PrSearchPage,
    PrSearchRequest, PrSearchState,
};

pub(crate) fn paged_search_string(request: &PrSearchRequest) -> Result<String, GhError> {
    let criteria = request.criteria();
    // Repeating an identity qualifier can broaden GitHub's legacy search.
    // Keep audience identity authoritative until the caller selects All.
    if (criteria.audience == PrAudience::Mine && criteria.author.is_some())
        || (criteria.audience == PrAudience::ReviewRequested
            && criteria.requested_reviewer.is_some())
    {
        return Err(GhError::BadArgument(
            "select the All audience before supplying an author or requested-reviewer override"
                .into(),
        ));
    }
    let mut text = String::from("is:pr sort:updated-desc");
    match criteria.state {
        PrSearchState::Open => text.push_str(" is:open"),
        PrSearchState::Merged => text.push_str(" is:merged"),
        PrSearchState::Closed => text.push_str(" is:closed is:unmerged"),
        PrSearchState::All => {}
    }
    for repo in request.repos() {
        text.push_str(&format!(" repo:{}/{}", repo.owner(), repo.name()));
    }
    match criteria.audience {
        PrAudience::Mine => text.push_str(" author:@me"),
        PrAudience::ReviewRequested => text.push_str(" review-requested:@me"),
        PrAudience::All => {}
    }
    for term in &criteria.terms {
        text.push(' ');
        text.push_str(&quoted(term)?);
    }
    for (qualifier, value) in [
        ("author", &criteria.author),
        ("assignee", &criteria.assignee),
        ("label", &criteria.label),
        ("head", &criteria.head),
        ("base", &criteria.base),
    ] {
        if let Some(value) = value {
            text.push_str(&format!(" {qualifier}:{}", quoted(value)?));
        }
    }
    if let Some(reviewer) = &criteria.requested_reviewer {
        let (qualifier, value) = match reviewer {
            PrRequestedReviewer::User(user) => ("review-requested", user),
            PrRequestedReviewer::Team(team) => ("team-review-requested", team),
        };
        text.push_str(&format!(" {qualifier}:{}", quoted(value)?));
    }
    if let Some(review) = criteria.review {
        let value = match review {
            PrReviewFilter::None => "none",
            PrReviewFilter::Required => "required",
            PrReviewFilter::Approved => "approved",
            PrReviewFilter::ChangesRequested => "changes_requested",
        };
        text.push_str(&format!(" review:{value}"));
    }
    if let Some(draft) = criteria.draft {
        text.push_str(&format!(" draft:{draft}"));
    }
    if let Some(status) = criteria.commit_status {
        let value = match status {
            PrCommitStatus::Pending => "pending",
            PrCommitStatus::Success => "success",
            PrCommitStatus::Failure => "failure",
        };
        text.push_str(&format!(" status:{value}"));
    }
    Ok(text)
}

/// GitHub documents quoting multiword values, but not escaping quotes in
/// this legacy search syntax. Refuse those values rather than widen scope.
fn quoted(value: &str) -> Result<String, GhError> {
    if value.trim().is_empty()
        || value
            .chars()
            .any(|c| c.is_control() || matches!(c, '"' | '\\'))
    {
        return Err(GhError::BadArgument(
            "search values must be nonempty and contain no quotes, backslashes or controls".into(),
        ));
    }
    Ok(format!("\"{value}\""))
}

pub(crate) fn paged_search_args(request: &PrSearchRequest) -> Result<Vec<String>, GhError> {
    let query = search_query(
        "first: $first, after: $after",
        ", $first: Int!, $after: String",
        "pageInfo { hasNextPage endCursor }",
    );
    let mut call = crate::system::Graphql::new(request.host(), &query)
        .string("q", &paged_search_string(request)?)
        .int("first", u64::from(request.page_size()));
    if let Some(cursor) = request.after() {
        call = call.string("after", cursor);
    }
    Ok(call.args())
}

pub(crate) fn parse_search_page(
    json: &str,
    request: &PrSearchRequest,
) -> Result<PrSearchPage, GhError> {
    let response: Response = serde_json::from_str(json).map_err(parse_error)?;
    if response.errors.as_ref().is_some_and(|e| !e.is_empty()) {
        return Err(GhError::Parse(format!(
            "GitHub search answered with errors: {}",
            api_messages(response.errors)
        )));
    }
    let search = response
        .data
        .and_then(|d| d.search)
        .ok_or_else(|| GhError::Parse("no search in the answer".into()))?;
    let raw_count = u32::try_from(search.nodes.len())
        .map_err(|_| GhError::Parse("search page length overflow".into()))?;
    let mut items = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for node in search.nodes {
        if !is_not_a_pull_request(&node) {
            let item = parse_search_node(node, request.host())?;
            if !request.repos().contains(&item.repo) {
                return Err(GhError::Parse(
                    "search returned an out-of-scope repository".into(),
                ));
            }
            if seen.insert((item.repo.clone(), item.pr.number)) {
                items.push(item);
            }
        }
    }
    request.finish_page(
        items,
        search.issue_count,
        raw_count,
        search.page_info.has_next_page,
        search.page_info.end_cursor,
    )
}

#[derive(Deserialize)]
struct Response {
    data: Option<Data>,
    errors: Option<Vec<ApiError>>,
}

#[derive(Deserialize)]
struct Data {
    search: Option<Connection>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Connection {
    issue_count: u32,
    nodes: Vec<serde_json::Value>,
    page_info: PageInfo,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
}

#[cfg(test)]
mod tests;
