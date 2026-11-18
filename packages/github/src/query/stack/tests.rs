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
