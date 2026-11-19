use daruda_github::fake::{FakeGh, GhCall, GhFailure, GhOp};
use daruda_github::{PrStack, PrState, StackEntry, StackPlace, StackRead};

use super::*;

const PARENT: u64 = 5;
const OTHER: u64 = 6;

fn repo() -> RepoId {
    RepoId::new("github.com", "o", "r")
}

/// GitHub where `PARENT` heads a stack of `members`.
fn gh_with_stack(members: &[u64]) -> FakeGh {
    let gh = FakeGh::new();
    gh.set_pr_stack(
        &repo(),
        PARENT,
        StackRead::Stacked(PrStack {
            number: 3,
            base: "main".into(),
            size: members.len() as u32,
            entries: members
                .iter()
                .zip(1..)
                .map(|(&number, position)| StackEntry {
                    position,
                    number,
                    title: String::new(),
                    state: PrState::Open,
                })
                .collect(),
        }),
    );
    gh
}

#[test]
fn a_pull_request_the_branch_had_owes_only_a_stacking_it_lacks() {
    // (label, stack asked for, the existing one's number, the parent's
    // stack, what is owed: its number and reason, or nothing)
    type Owed = Option<(Option<u64>, String)>;
    type Row = (&'static str, Option<u64>, Option<u64>, &'static [u64], Owed);
    let cases: [Row; 4] = [
        ("no stacking asked", None, Some(OTHER), &[PARENT], None),
        (
            "already in that stack",
            Some(PARENT),
            Some(OTHER),
            &[PARENT, OTHER],
            None,
        ),
        (
            "not in that stack",
            Some(PARENT),
            Some(OTHER),
            &[PARENT],
            Some((Some(OTHER), s::github_pr_stack_existing())),
        ),
        (
            "its number is not known",
            Some(PARENT),
            None,
            &[PARENT],
            Some((None, s::github_pr_stack_no_number())),
        ),
    ];
    for (label, stack_on, existing, members, expected) in cases {
        let gh = gh_with_stack(members);
        let owed = existing_unstacked(&gh, &repo(), stack_on, existing);
        assert_eq!(
            owed.map(|left| (left.number, left.reason)),
            expected,
            "{label}"
        );
        // Told, never stacked here: the pull request is not this form's.
        assert_eq!(gh.count(GhOp::StackPr), 0, "{label}");
    }
}

#[test]
fn a_stack_that_cannot_be_read_leaves_the_stacking_owed() {
    let gh = gh_with_stack(&[PARENT, OTHER]);
    gh.fail(
        GhOp::PrStack,
        GhFailure::Exit {
            code: Some(1),
            stderr: "boom".into(),
        },
    );
    let owed = existing_unstacked(&gh, &repo(), Some(PARENT), Some(OTHER)).unwrap();
    assert_eq!((owed.parent, owed.number), (PARENT, Some(OTHER)));
    assert!(owed.reason.contains("boom"), "{}", owed.reason);
}

#[test]
fn a_created_pull_request_is_stacked_by_the_number_in_its_url() {
    let place = StackPlace::New { parent: PARENT };
    let url = "https://github.com/o/r/pull/6";

    let gh = FakeGh::new();
    assert_eq!(
        stack_created(&gh, &repo(), PARENT, place, url, "feat"),
        None
    );
    assert!(gh.calls().contains(&GhCall::StackPr {
        repo: repo(),
        place,
        number: OTHER,
    }));

    // GitHub refused: the pull request stays, named, to be stacked again.
    let gh = FakeGh::new();
    gh.fail(
        GhOp::StackPr,
        GhFailure::Exit {
            code: Some(1),
            stderr: "boom".into(),
        },
    );
    let left = stack_created(&gh, &repo(), PARENT, place, url, "feat").unwrap();
    assert_eq!((left.parent, left.number), (PARENT, Some(OTHER)));
    assert!(left.reason.contains("boom"), "{}", left.reason);

    // An answer with no number in it: nothing to stack, and none asked.
    let gh = FakeGh::new();
    let url = "https://github.com/o/r/pulls";
    let left = stack_created(&gh, &repo(), PARENT, place, url, "feat").unwrap();
    assert_eq!(left.number, None);
    assert_eq!(left.reason, s::github_pr_stack_no_number());
    assert_eq!(gh.count(GhOp::StackPr), 0);
}
