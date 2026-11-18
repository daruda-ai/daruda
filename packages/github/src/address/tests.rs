use super::*;

#[test]
fn references_name_a_pull_request_and_what_in_it() {
    let gh = |owner: &str, name: &str| Some(RepoId::new("github.com", owner, name));
    // (input, repo, host unknown, number, target), or None when refused
    type Row = (
        &'static str,
        Option<(Option<RepoId>, bool, u64, Option<PrTarget>)>,
    );
    let rows: Vec<Row> = vec![
        (
            "https://github.com/cli/cli/pull/15003",
            Some((gh("cli", "cli"), false, 15003, None)),
        ),
        (
            "https://ghe.corp/o/r/pull/7/files",
            Some((
                Some(RepoId::new("ghe.corp", "o", "r")),
                false,
                7,
                Some(PrTarget::Files),
            )),
        ),
        (
            "https://github.com/o/r/pull/7/files#discussion_r123",
            Some((gh("o", "r"), false, 7, Some(PrTarget::ReviewComment(123)))),
        ),
        (
            "https://github.com/o/r/pull/7#issuecomment-9",
            Some((gh("o", "r"), false, 7, Some(PrTarget::Comment(9)))),
        ),
        (
            "https://github.com/o/r/pull/7/checks?x=1",
            Some((gh("o", "r"), false, 7, Some(PrTarget::Checks))),
        ),
        (
            "  https://github.com/o/r/pull/7/  ",
            Some((gh("o", "r"), false, 7, None)),
        ),
        ("O/R#12", Some((gh("o", "r"), true, 12, None))),
        (
            "HTTPS://www.github.com/o/r/pull/7",
            Some((gh("o", "r"), false, 7, None)),
        ),
        (
            "https://ghe.corp:8443/o/r/pull/7/commits",
            Some((
                Some(RepoId::new("ghe.corp", "o", "r")),
                false,
                7,
                Some(PrTarget::Commits),
            )),
        ),
        (
            "https://github.com/o/r/pull/7#discussion_rX",
            Some((gh("o", "r"), false, 7, None)),
        ),
        (
            "https://github.com/o/r/pull/7#pullrequestreview-1",
            Some((gh("o", "r"), false, 7, None)),
        ),
        ("https://github.com/o/r/pulls/7", None),
        ("#12", Some((None, true, 12, None))),
        ("12", Some((None, true, 12, None))),
        ("https://github.com/o/r/issues/7", None),
        ("https://github.com/o/r/pull/x", None),
        ("javascript:alert(1)", None),
        ("o/r/x#3", None),
        ("#0", None),
        ("", None),
        ("12a", None),
    ];
    for (input, expected) in rows {
        let got = parse_pr_address(input).map(|address| {
            (
                address.repo,
                address.host_unknown,
                address.number,
                address.target,
            )
        });
        assert_eq!(got, expected, "{input:?}");
    }
}
