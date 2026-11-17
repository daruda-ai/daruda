//! Recorded on 2026-11-17 with `search_page.graphql`, `first=2` and
//! `q=is:pr sort:updated-desc repo:cli/cli`; the second request passes the
//! first answer's endCursor. These public-repository fixtures need no login in tests.

use super::*;
use crate::{PrSearchContinuation, PrSearchCriteria, RepoId};

fn request(criteria: PrSearchCriteria) -> PrSearchRequest {
    PrSearchRequest::new(
        "github.com",
        &[RepoId::new("github.com", "cli", "cli")],
        criteria,
        2,
    )
    .unwrap()
}

#[test]
fn state_and_audience_are_independent_and_closed_excludes_merges() {
    let states = [
        (PrSearchState::Open, " is:open"),
        (PrSearchState::Merged, " is:merged"),
        (PrSearchState::Closed, " is:closed is:unmerged"),
        (PrSearchState::All, ""),
    ];
    let audiences = [
        (PrAudience::Mine, " author:@me"),
        (PrAudience::ReviewRequested, " review-requested:@me"),
        (PrAudience::All, ""),
    ];
    for (state, state_text) in states {
        for (audience, audience_text) in audiences {
            let request = request(PrSearchCriteria {
                state,
                audience,
                ..Default::default()
            });
            assert_eq!(
                paged_search_string(&request).unwrap(),
                format!("is:pr sort:updated-desc{state_text} repo:cli/cli{audience_text}"),
                "{state:?}, {audience:?}"
            );
        }
    }
}

#[test]
fn qualifiers_and_literal_terms_cannot_widen_the_scope() {
    let request = request(PrSearchCriteria {
        audience: PrAudience::All,
        terms: vec!["repo:other/repo".into(), "OR".into(), "한글 phrase".into()],
        author: Some("octocat".into()),
        assignee: Some("@me".into()),
        label: Some("help wanted".into()),
        requested_reviewer: Some(PrRequestedReviewer::Team("org/team".into())),
        review: Some(PrReviewFilter::ChangesRequested),
        draft: Some(false),
        commit_status: Some(PrCommitStatus::Failure),
        head: Some("123".into()),
        base: Some("main".into()),
        ..Default::default()
    });
    assert_eq!(
        paged_search_string(&request).unwrap(),
        "is:pr sort:updated-desc is:open repo:cli/cli \"repo:other/repo\" \"OR\" \"한글 phrase\" author:\"octocat\" assignee:\"@me\" label:\"help wanted\" head:\"123\" base:\"main\" team-review-requested:\"org/team\" review:changes_requested draft:false status:failure"
    );
    let args = paged_search_args(&request).unwrap();
    assert!(
        args.windows(2)
            .any(|a| a[0] == "-f" && a[1].starts_with("q="))
    );
    assert!(args.windows(2).any(|a| a == ["-F", "first=2"]));
    assert!(
        !args.iter().any(|a| a.starts_with("after=")),
        "null cursor is omitted"
    );
    let query = args.iter().find_map(|a| a.strip_prefix("query=")).unwrap();
    assert_eq!(
        query,
        include_str!("../../../tests/fixtures/search_page.graphql").trim()
    );
}

#[test]
fn partial_errors_and_broken_pagination_never_claim_complete_results() {
    let request = request(PrSearchCriteria::default());
    let answer = |nodes: &str, info: &str| {
        format!(r#"{{"data":{{"search":{{"issueCount":2,"nodes":{nodes},"pageInfo":{info}}}}}}}"#)
    };
    let cases = [
        ("missing cursor", answer("[null]", r#"{"hasNextPage":true,"endCursor":null}"#)),
        ("empty next page", answer("[]", r#"{"hasNextPage":true,"endCursor":"next"}"#)),
        ("missing page info", r#"{"data":{"search":{"issueCount":0,"nodes":[]}}}"#.into()),
        ("partial GraphQL error", r#"{"data":{"search":{"issueCount":0,"nodes":[],"pageInfo":{"hasNextPage":false}}},"errors":[{"message":"resource not accessible"}]}"#.into()),
        ("missing search", r#"{"data":null}"#.into()),
    ];
    for (label, json) in cases {
        assert!(
            matches!(parse_search_page(&json, &request), Err(GhError::Parse(_))),
            "{label}"
        );
    }
    let missing = answer("[null,{}]", r#"{"hasNextPage":false}"#);
    assert_eq!(
        parse_search_page(&missing, &request).unwrap().continuation,
        PrSearchContinuation::Incomplete
    );
}

#[test]
fn recorded_pages_advance_without_reusing_the_first_page() {
    let request = request(PrSearchCriteria {
        audience: PrAudience::All,
        state: PrSearchState::All,
        ..Default::default()
    });
    let first = parse_search_page(
        include_str!("../../../tests/fixtures/search_page_first.json"),
        &request,
    )
    .unwrap();
    let PrSearchContinuation::More(next) = first.continuation else {
        panic!("next page");
    };
    assert!(next.after().is_some());
    let args = paged_search_args(&next).unwrap();
    assert!(
        args.windows(2)
            .any(|a| a[0] == "-f" && a[1] == format!("after={}", next.after().unwrap()))
    );
    let second = parse_search_page(
        include_str!("../../../tests/fixtures/search_page_next.json"),
        &next,
    )
    .unwrap();
    assert_eq!(first.total_count, second.total_count);
    assert_eq!((first.items.len(), second.items.len()), (2, 2));
    for item in &second.items {
        assert!(
            !first
                .items
                .iter()
                .any(|old| old.repo == item.repo && old.pr.number == item.pr.number)
        );
    }
    let PrSearchContinuation::More(third) = second.continuation else {
        panic!("third page");
    };
    assert_ne!(third.after(), next.after());
}

#[test]
fn duplicate_and_out_of_scope_nodes_cannot_claim_full_coverage() {
    let request = request(PrSearchCriteria {
        audience: PrAudience::All,
        ..Default::default()
    });
    let original: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/search_page_first.json"
    ))
    .unwrap();
    let mut duplicate = original.clone();
    let search = &mut duplicate["data"]["search"];
    search["issueCount"] = 2.into();
    search["nodes"][1] = search["nodes"][0].clone();
    search["pageInfo"]["hasNextPage"] = false.into();
    let page = parse_search_page(&duplicate.to_string(), &request).unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.continuation, PrSearchContinuation::Incomplete);

    let mut foreign = original.clone();
    foreign["data"]["search"]["nodes"][0]["repository"]["nameWithOwner"] = "other/repo".into();
    assert!(parse_search_page(&foreign.to_string(), &request).is_err());
    let small = PrSearchRequest::new(
        request.host(),
        request.repos(),
        request.criteria().clone(),
        1,
    )
    .unwrap();
    assert!(parse_search_page(&original.to_string(), &small).is_err());
}
