//! `cli/cli` #15003's answer was read with `gh api graphql` on 2026-11-18
//! (not stacked). The stacked answer is shaped as GitHub's schema
//! documents it: no stacked pull request was at hand to record.

use super::*;

#[test]
fn a_pull_request_in_no_stack_reads_as_such() {
    let real = r#"{"data":{"repository":{"pullRequest":{"stack":null}}}}"#;
    assert_eq!(parse_stack(real).unwrap(), StackRead::NotStacked);
}

#[test]
fn a_stack_reads_in_order_with_what_was_not_listed() {
    let answer = r#"{"data":{"repository":{"pullRequest":{"stack":{"number":3,"size":4,"baseRefName":"main","entries":{"totalCount":4,"nodes":[
        {"position":2,"pullRequest":{"number":12,"title":"Second","state":"OPEN","isDraft":true}},
        {"position":1,"pullRequest":{"number":11,"title":"First","state":"MERGED","isDraft":false}},
        {"position":3,"pullRequest":null}
    ]}}}}}}"#;
    let StackRead::Stacked(stack) = parse_stack(answer).unwrap() else {
        panic!("stacked");
    };
    let order: Vec<(u32, u64, PrState)> = stack
        .entries
        .iter()
        .map(|entry| (entry.position, entry.number, entry.state))
        .collect();
    assert_eq!(order, [(1, 11, PrState::Merged), (2, 12, PrState::Draft)]);
    assert_eq!(
        (stack.number, stack.base.as_str(), stack.more()),
        (3, "main", 2)
    );
    assert!(stack_query().contains("stack {"));
}

#[test]
fn a_server_without_stacks_is_told_apart_from_a_failure() {
    assert!(lacks_stacks(
        "gh: Field 'stack' doesn't exist on type 'PullRequest'"
    ));
    assert!(!lacks_stacks("gh: Could not resolve to a PullRequest"));
    // Another field's refusal mentioning a stack is not this.
    assert!(!lacks_stacks(
        "gh: Field 'stackTrace' doesn't exist on type 'Commit'"
    ));
}

#[test]
fn the_range_below_takes_the_open_pull_requests_up_to_this_one() {
    let entry = |position, number, state| StackEntry {
        position,
        number,
        title: String::new(),
        state,
    };
    let stack = PrStack {
        number: 1,
        base: "main".into(),
        size: 4,
        entries: vec![
            entry(1, 10, PrState::Merged),
            entry(2, 11, PrState::Open),
            entry(3, 12, PrState::Draft),
            entry(4, 13, PrState::Open),
        ],
    };
    assert_eq!(
        stack.range_below(13),
        Some(vec![11, 13]),
        "merged and draft left out"
    );
    assert_eq!(stack.range_below(11), None, "nothing open below");
    assert_eq!(stack.range_below(99), None, "not listed");
}

fn stack_of(number: u64, size: u32, listed: &[u64]) -> StackRead {
    StackRead::Stacked(PrStack {
        number,
        base: "main".into(),
        size,
        entries: listed
            .iter()
            .zip(1..)
            .map(|(&number, position)| StackEntry {
                position,
                number,
                title: String::new(),
                state: PrState::Open,
            })
            .collect(),
    })
}

/// Only the top of a stack takes another pull request: GitHub appends,
/// and has nothing that inserts.
#[test]
fn a_new_pull_request_goes_on_top_or_nowhere() {
    let cases = [
        (
            "the parent is in no stack: the two make one",
            StackRead::NotStacked,
            Ok(StackPlace::New { parent: 12 }),
        ),
        (
            "the parent is the top of its stack",
            stack_of(3, 2, &[11, 12]),
            Ok(StackPlace::Top { stack: 3 }),
        ),
        (
            "the parent has one above it",
            stack_of(3, 3, &[11, 12, 13]),
            Err(NoStacking::NotTop),
        ),
        (
            // More entries than were read: the top is not known to be it.
            "the stack is longer than what was listed",
            stack_of(3, 5, &[11, 12]),
            Err(NoStacking::NotTop),
        ),
        (
            "the server has no stacks",
            StackRead::Unsupported,
            Err(NoStacking::Unsupported),
        ),
    ];
    for (label, read, expected) in cases {
        assert_eq!(read.place_on(12), expected, "{label}");
    }
}

/// A new stack names both pull requests bottom first; an append names
/// only the new one, on the stack's own path.
#[test]
fn the_request_is_the_one_githubs_reference_describes() {
    let repo = crate::RepoId::new("github.com", "o", "r");
    let (args, body) = stack_request(&repo, StackPlace::New { parent: 12 }, 13);
    assert_eq!(
        args,
        [
            "api",
            "--hostname",
            "github.com",
            "--method",
            "POST",
            "-H",
            "X-GitHub-Api-Version: 2026-03-10",
            "repos/o/r/stacks",
            "--input",
            "-"
        ]
    );
    assert_eq!(body, r#"{"pull_requests":[12,13]}"#);
    let (args, body) = stack_request(&repo, StackPlace::Top { stack: 3 }, 13);
    assert_eq!(args[7], "repos/o/r/stacks/3/add");
    assert_eq!(body, r#"{"pull_requests":[13]}"#);
}
