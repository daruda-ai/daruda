use daruda_github::{
    Gh, GhError, PrAudience, PrRequestedReviewer, PrSearchContinuation, PrSearchCriteria,
    PrSearchRequest, RepoId, SystemGh,
};

#[test]
fn repository_chunks_are_canonical_and_bounded_before_sending() {
    let repo = |host: &str, name: &str| RepoId::new(host, "o", name);
    let criteria = PrSearchCriteria::default();
    let request = PrSearchRequest::new(
        "GITHUB.COM",
        &[
            repo("github.com", "b"),
            repo("github.com", "a"),
            repo("GITHUB.COM", "b"),
        ],
        criteria.clone(),
        50,
    )
    .unwrap();
    assert_eq!(request.host(), "github.com");
    assert_eq!(
        request.repos(),
        [repo("github.com", "a"), repo("github.com", "b")]
    );
    assert_eq!(request.after(), None);

    let many: Vec<_> = (0..21)
        .map(|n| repo("github.com", &format!("r{n}")))
        .collect();
    let cases = [
        ("another host", vec![repo("ghe.corp", "r")], 50),
        ("too many repositories", many, 50),
        ("zero size", vec![], 0),
        ("unbounded page", vec![], 51),
        ("empty repository", vec![repo("github.com", "")], 50),
        (
            "search syntax in repository",
            vec![repo("github.com", "r repo:other/r")],
            50,
        ),
    ];
    for (label, repos, size) in cases {
        assert!(
            matches!(
                PrSearchRequest::new("github.com", &repos, criteria.clone(), size),
                Err(GhError::BadArgument(_))
            ),
            "{label}"
        );
    }
    let empty = PrSearchRequest::new("github.com", &[], criteria, 50).unwrap();
    let page = SystemGh.search_prs_page(&empty).unwrap();
    assert!(page.items.is_empty());
    assert_eq!(page.total_count, 0);
    assert_eq!(page.continuation, PrSearchContinuation::Complete);
}

#[test]
fn unsupported_escaping_is_reported_instead_of_changing_search_scope() {
    for value in ["", " ", "a\" repo:other/r", "a\\b", "a\nb", "a\0b"] {
        let criteria = PrSearchCriteria {
            terms: vec![value.into()],
            ..Default::default()
        };
        assert!(
            matches!(
                PrSearchRequest::new("github.com", &[], criteria, 50),
                Err(GhError::BadArgument(_))
            ),
            "{value:?}"
        );
        let criteria = PrSearchCriteria {
            label: Some(value.into()),
            ..Default::default()
        };
        assert!(
            matches!(
                PrSearchRequest::new("github.com", &[], criteria, 50),
                Err(GhError::BadArgument(_))
            ),
            "label {value:?}"
        );
    }
}

#[test]
fn identity_facets_cannot_override_the_audience() {
    let cases = [
        (PrAudience::Mine, Some("other".into()), None),
        (
            PrAudience::ReviewRequested,
            None,
            Some(PrRequestedReviewer::User("other".into())),
        ),
        (
            PrAudience::ReviewRequested,
            None,
            Some(PrRequestedReviewer::Team("org/team".into())),
        ),
    ];
    for (audience, author, requested_reviewer) in cases {
        let criteria = PrSearchCriteria {
            audience,
            author,
            requested_reviewer,
            ..Default::default()
        };
        assert!(
            matches!(
                PrSearchRequest::new("github.com", &[], criteria, 50),
                Err(GhError::BadArgument(_))
            ),
            "{audience:?}"
        );
    }
}
