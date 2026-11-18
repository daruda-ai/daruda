use super::*;

#[cfg(unix)]
#[test]
fn a_failed_lookup_is_excused_only_for_numbers_github_cannot_resolve() {
    use std::os::unix::process::ExitStatusExt as _;

    let captured = |code: i32, stdout: &str| Captured {
        status: std::process::ExitStatus::from_raw(code << 8),
        stdout: stdout.as_bytes().to_vec(),
        stderr: b"gh: Could not resolve".to_vec(),
    };
    let not_found = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/prs_number_not_found.json"),
    )
    .unwrap();
    let branches = ["a".to_owned()];
    let answer = read_lane_answer(&captured(1, &not_found), &branches, &[9, 404])
        .expect("the rest of the answer stands");
    assert_eq!(answer.by_number[&404], None);
    assert!(answer.by_number[&9].is_some());

    // (label, exit code, stdout)
    let failures = [
        ("nothing printed", 1, ""),
        (
            "another error",
            1,
            r#"{"data":null,"errors":[{"type":"RATE_LIMITED","message":"x"}]}"#,
        ),
        ("a login failure prints nothing", 4, ""),
    ];
    for (label, code, stdout) in failures {
        match read_lane_answer(&captured(code, stdout), &branches, &[404]) {
            Err(GhError::Exit { code: got, .. }) => assert_eq!(got, Some(code), "{label}"),
            other => panic!("{label}: expected the exit, got {other:?}"),
        }
    }
}

#[test]
fn numbers_ride_with_branches_and_need_no_branch_to_be_asked() {
    let many: Vec<String> = (0..=BRANCHES_PER_REQUEST)
        .map(|i| format!("b{i}"))
        .collect();
    /// (label, branches, numbers, (branches, numbers) per request)
    type Case<'a> = (&'a str, &'a [String], &'a [u64], Vec<(usize, usize)>);
    let cases: [Case; 4] = [
        ("nothing", &[], &[], vec![]),
        ("numbers alone", &[], &[7, 9], vec![(0, 2)]),
        ("both in one", &many[..2], &[7], vec![(2, 1)]),
        (
            "numbers in the first of two",
            &many,
            &[7],
            vec![(BRANCHES_PER_REQUEST, 1), (1, 0)],
        ),
    ];
    for (label, branches, numbers, expected) in cases {
        let sizes: Vec<(usize, usize)> = lane_requests(branches, numbers)
            .into_iter()
            .map(|(b, n)| (b.len(), n.len()))
            .collect();
        assert_eq!(sizes, expected, "{label}");
    }
}
