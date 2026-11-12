use daruda_github::link::pick;
use daruda_github::{CheckRollup, PrState, PrSummary};

fn pr(number: u64, head: &str, state: PrState, cross_repo: bool, updated: &str) -> PrSummary {
    PrSummary {
        number,
        url: format!("https://github.com/o/r/pull/{number}"),
        title: String::new(),
        state,
        head_branch: head.into(),
        cross_repo,
        checks: CheckRollup::None,
        failed: Vec::new(),
        review: None,
        updated_at: updated.into(),
    }
}

#[test]
fn a_branch_links_to_its_own_pull_request() {
    use PrState::*;
    let cases: Vec<(&str, &str, Vec<PrSummary>, Option<u64>)> = vec![
        ("no candidates", "feat", vec![], None),
        (
            "same-repo open PR",
            "feat",
            vec![pr(1, "feat", Open, false, "2026-01-01")],
            Some(1),
        ),
        (
            "a fork's PR with the same head name",
            "feat",
            vec![pr(1, "feat", Open, true, "2026-01-01")],
            None,
        ),
        (
            "the default branch has no PR of its own",
            "main",
            vec![pr(1, "main", Open, false, "2026-01-01")],
            None,
        ),
        (
            "branch names are case-sensitive",
            "Feat",
            vec![pr(1, "feat", Open, false, "2026-01-01")],
            None,
        ),
        (
            "open beats a more recent closed PR",
            "feat",
            vec![
                pr(1, "feat", Open, false, "2026-01-01"),
                pr(2, "feat", Closed, false, "2026-02-01"),
            ],
            Some(1),
        ),
        (
            "draft counts as open",
            "feat",
            vec![
                pr(1, "feat", Merged, false, "2026-02-01"),
                pr(2, "feat", Draft, false, "2026-01-01"),
            ],
            Some(2),
        ),
        (
            "among closed PRs the latest wins",
            "feat",
            vec![
                pr(1, "feat", Closed, false, "2026-01-01"),
                pr(2, "feat", Merged, false, "2026-03-01"),
            ],
            Some(2),
        ),
    ];
    for (label, branch, candidates, expected) in cases {
        assert_eq!(
            pick(branch, "main", &candidates).map(|p| p.number),
            expected,
            "{label}"
        );
    }
}
