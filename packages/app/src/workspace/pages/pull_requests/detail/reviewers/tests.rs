use super::*;

/// The names read, or the first that cannot be a reviewer.
type Read<'a> = Result<Vec<&'a str>, &'a str>;

#[test]
fn names_are_split_deduplicated_and_checked() {
    // (label, typed, names or the first bad one)
    let cases: [(&str, &str, Read); 5] = [
        (
            "commas and spaces",
            "alice, bob  carol",
            Ok(vec!["alice", "bob", "carol"]),
        ),
        ("an @ is dropped", "@alice", Ok(vec!["alice"])),
        ("a team", "org/core", Ok(vec!["org/core"])),
        ("the same twice", "Alice alice", Ok(vec!["Alice"])),
        ("a flag", "alice --admin", Err("--admin")),
    ];
    for (label, typed, expected) in cases {
        let expected = expected
            .map(|names| names.into_iter().map(str::to_owned).collect::<Vec<_>>())
            .map_err(str::to_owned);
        assert_eq!(reviewer_names(typed), expected, "{label}");
    }
}
