use super::*;
use crate::fake::{FakeGh, GhOp};
use crate::{PrStack, StackEntry, StackRead};

fn stacked(numbers: &[u64]) -> StackRead {
    let entries = numbers
        .iter()
        .zip(1..)
        .map(|(number, position)| StackEntry {
            position,
            number: *number,
            title: String::new(),
            state: crate::PrState::Open,
        })
        .collect::<Vec<_>>();
    StackRead::Stacked(PrStack {
        number: 1,
        base: "main".into(),
        size: entries.len() as u32,
        entries,
    })
}

#[test]
fn the_stack_shown_is_checked_again_before_it_is_sent() {
    let repo = RepoId::new("github.com", "o", "r");
    // (label, stack now, range shown, sent)
    let cases = [
        ("unchanged", stacked(&[6, 7]), Some(vec![6, 7]), true),
        ("grown below", stacked(&[5, 6, 7]), Some(vec![6, 7]), false),
        (
            "no longer stacked",
            StackRead::NotStacked,
            Some(vec![6, 7]),
            false,
        ),
        ("alone: not read", StackRead::NotStacked, None, true),
    ];
    for (label, now, shown, sent) in cases {
        let gh = FakeGh::default();
        gh.set_pr_stack(&repo, 7, now);
        let result = enable_auto_merge_shown(
            &gh,
            &repo,
            7,
            "PR_7",
            MergeMethod::Squash,
            "abc",
            shown.as_deref(),
        );
        assert_eq!(gh.count(GhOp::EnableAutoMerge) == 1, sent, "{label}");
        if !sent {
            assert!(matches!(result, Err(GhError::StackChanged)), "{label}");
        }
    }
}
