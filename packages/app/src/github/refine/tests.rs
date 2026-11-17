use daruda_github::{PrAudience, PrCommitStatus, PrRequestedReviewer, PrReviewFilter};

use super::*;

#[test]
fn text_reads_as_words_and_known_facets_and_back() {
    let refine = Refine::parse(
        "  sidebar  Author:octo label:ui review:approved draft:false status:failure  docs ",
    );
    assert_eq!(refine.terms, ["sidebar", "docs"]);
    assert_eq!(
        refine.facets,
        [
            Facet::Author("octo".into()),
            Facet::Label("ui".into()),
            Facet::Review(PrReviewFilter::Approved),
            Facet::Draft(false),
            Facet::Status(PrCommitStatus::Failure),
        ]
    );
    assert_eq!(
        Refine::parse(&refine.text()),
        refine,
        "the text reads back the same"
    );
    // (label, text, words, facets)
    let cases: [(&str, &str, &[&str], usize); 5] = [
        ("an unknown key is a word", "foo:bar", &["foo:bar"], 0),
        (
            "an unknown value is a word",
            "review:maybe",
            &["review:maybe"],
            0,
        ),
        ("an empty value is a word", "label:", &["label:"], 0),
        ("one of a kind, the later wins", "label:a label:b", &[], 1),
        (
            "a user and a team request share one",
            "reviewer:a team:org/t",
            &[],
            1,
        ),
    ];
    for (label, text, words, facets) in cases {
        let refine = Refine::parse(text);
        assert_eq!(refine.terms, words, "{label}");
        assert_eq!(refine.facets.len(), facets, "{label}");
    }
    assert!(Refine::parse("   ").is_empty());
}

#[test]
fn removing_a_facet_keeps_the_rest() {
    let refine = Refine::parse("fix label:ui author:octo");
    assert_eq!(refine.without_facet(0).text(), "fix author:octo");
    assert_eq!(
        refine.without_facet(9),
        refine,
        "out of range removes nothing"
    );
}

#[test]
fn a_refinement_says_what_keeps_it_from_a_tab() {
    // (label, text, audience, problem)
    let cases = [
        ("words go anywhere", "fix", PrAudience::Mine, None),
        (
            "author beside Mine",
            "author:octo",
            PrAudience::Mine,
            Some(RefineProblem::NeedsAllTab),
        ),
        ("author beside All", "author:octo", PrAudience::All, None),
        (
            "a team beside Review requested",
            "team:org/t",
            PrAudience::ReviewRequested,
            Some(RefineProblem::NeedsAllTab),
        ),
        (
            "a quote",
            "\"fix",
            PrAudience::All,
            Some(RefineProblem::Unsupported),
        ),
        (
            "a backslash in a facet",
            "label:a\\b",
            PrAudience::All,
            Some(RefineProblem::Unsupported),
        ),
    ];
    for (label, text, audience, problem) in cases {
        assert_eq!(Refine::parse(text).problem(audience), problem, "{label}");
    }
}

#[test]
fn a_refinement_fills_the_typed_criteria() {
    let mut criteria = PrSearchCriteria {
        audience: PrAudience::All,
        ..Default::default()
    };
    Refine::parse("fix team:org/t assignee:me draft:true").apply(&mut criteria);
    assert_eq!(criteria.terms, ["fix"]);
    assert_eq!(
        criteria.requested_reviewer,
        Some(PrRequestedReviewer::Team("org/t".into()))
    );
    assert_eq!(criteria.assignee.as_deref(), Some("me"));
    assert_eq!(criteria.draft, Some(true));
    assert_eq!(
        criteria.audience,
        PrAudience::All,
        "the tab's audience stays"
    );
}
