use super::*;

fn request() -> PrSearchRequest {
    PrSearchRequest::new(
        "github.com",
        &[RepoId::new("github.com", "o", "r")],
        PrSearchCriteria::default(),
        SEARCH_PAGE_MAX,
    )
    .unwrap()
}

#[test]
fn coverage_survives_page_boundaries_and_stops_at_the_search_cap() {
    // (label, already scanned, previous omission, total, has next, outcome)
    let cases = [
        ("empty", 0, false, 0, false, PrSearchContinuation::Complete),
        (
            "short connection",
            0,
            false,
            1,
            false,
            PrSearchContinuation::Incomplete,
        ),
        (
            "earlier missing node",
            50,
            true,
            50,
            false,
            PrSearchContinuation::Incomplete,
        ),
        (
            "last page",
            50,
            false,
            50,
            false,
            PrSearchContinuation::Complete,
        ),
        (
            "exactly the cap",
            1000,
            false,
            1000,
            false,
            PrSearchContinuation::Complete,
        ),
        (
            "more than the cap",
            1000,
            false,
            1001,
            false,
            PrSearchContinuation::SearchLimit,
        ),
        (
            "provider still advertises a next page at the cap",
            1000,
            false,
            1000,
            true,
            PrSearchContinuation::SearchLimit,
        ),
    ];
    for (label, scanned, incomplete, total, has_next, expected) in cases {
        let mut request = request();
        request.scanned = scanned;
        request.incomplete = incomplete;
        let page = request
            .finish_page(vec![], total, 0, has_next, None)
            .unwrap();
        assert_eq!(page.continuation, expected, "{label}");
    }
}

#[test]
fn a_continuation_keeps_its_scope_and_remembers_missing_nodes() {
    let request = request();
    let page = request
        .finish_page(vec![], 80, 50, true, Some("next".into()))
        .unwrap();
    let PrSearchContinuation::More(next) = page.continuation else {
        panic!("next page");
    };
    assert_eq!(next.host(), request.host());
    assert_eq!(next.repos(), request.repos());
    assert_eq!(next.criteria(), request.criteria());
    assert_eq!(next.page_size(), request.page_size());
    assert_eq!(next.after(), Some("next"));
    assert_eq!(next.scanned, 50);
    assert!(next.incomplete);
    for cursor in ["next", ""] {
        assert!(
            next.finish_page(vec![], 80, 1, true, Some(cursor.into()))
                .is_err()
        );
    }
}
