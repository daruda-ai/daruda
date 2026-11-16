use daruda_github::{CheckRollup, PrCheckout, PrListItem, PrState, PrSummary, RepoId};

fn item(cross_repo: bool) -> PrListItem {
    PrListItem {
        repo: RepoId::new("github.com", "o", "r"),
        pr: PrSummary {
            number: 9,
            url: "https://github.com/o/r/pull/9".into(),
            title: "t".into(),
            state: PrState::Open,
            head_branch: "fix/scroll".into(),
            cross_repo,
            checks: CheckRollup::None,
            failed: Vec::new(),
            review: None,
            updated_at: String::new(),
        },
        author: Some("me".into()),
        head_owner: None,
        base_branch: "main".into(),
        comments: 0,
    }
}

#[test]
fn a_pull_request_is_fetched_to_a_name_its_branch_starts_from() {
    // (label, cross_repo, refspec, branch, start, review mark)
    let cases = [
        (
            "same repository: its own branch",
            false,
            "+refs/heads/fix/scroll:refs/remotes/up/fix/scroll",
            "fix/scroll",
            "refs/remotes/up/fix/scroll",
            None,
        ),
        (
            "fork: a review branch marked with its PR",
            true,
            "+refs/pull/9/head:refs/daruda/pr/9",
            "pr-9",
            "refs/daruda/pr/9",
            Some("github.com/o/r#9"),
        ),
    ];
    for (label, cross_repo, refspec, branch, start, review) in cases {
        assert_eq!(
            PrCheckout::of(&item(cross_repo), "up"),
            PrCheckout {
                remote: "up".into(),
                refspec: refspec.into(),
                branch: branch.into(),
                start: start.into(),
                review: review.map(str::to_owned),
            },
            "{label}"
        );
    }
}
