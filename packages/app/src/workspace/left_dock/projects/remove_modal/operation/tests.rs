use super::*;
use crate::lane::git::{FakeFailure, FakeGit, FakeRepo, Git, GitCall, GitOp, LaneRemovalState};
use std::path::Path;

const ROOT: &str = "/repo";
const LANE: &str = "/repo-feature";

#[test]
fn unregistered_folder_requires_explicit_opt_in_and_preserves_branch_for_list_only() {
    for delete_folder in [false, true] {
        let git = FakeGit::new();
        git.add_repo(FakeRepo::at(ROOT).lane_removal_state(LANE, LaneRemovalState::Unregistered));
        let outcome = run(
            git.open(Path::new(ROOT)).as_ref(),
            Path::new(LANE),
            Action::Worktree {
                expected: LaneRemovalState::Unregistered,
                delete_folder,
                force: false,
                branch: Some("feature".into()),
            },
        )
        .unwrap();
        assert!(matches!(outcome, Outcome::Removed { branch_error: None }));
        let calls = git.calls();
        assert_eq!(calls.len(), usize::from(delete_folder));
        if delete_folder {
            assert!(
                matches!(&calls[0], GitCall::RemoveOrphanedLane { path, .. } if path == Path::new(LANE))
            );
        }
    }
}

#[test]
fn changed_registration_requires_a_new_confirmation() {
    let git = FakeGit::new();
    git.add_repo(FakeRepo::at(ROOT).lane_removal_state(LANE, LaneRemovalState::Unregistered));
    let outcome = run(
        git.open(Path::new(ROOT)).as_ref(),
        Path::new(LANE),
        Action::Worktree {
            expected: LaneRemovalState::Registered,
            delete_folder: false,
            force: true,
            branch: Some("feature".into()),
        },
    )
    .unwrap();
    assert!(matches!(
        outcome,
        Outcome::StateChanged(LaneRemovalState::Unregistered)
    ));
    assert!(git.calls().is_empty());
}

#[test]
fn branch_failure_keeps_worktree_success_and_retry_only_deletes_the_branch() {
    let git = FakeGit::new();
    git.add_repo(FakeRepo::at(ROOT).local_branches(["feature"]));
    git.fail(
        ROOT,
        GitOp::DeleteBranch,
        FakeFailure::Exit {
            code: Some(1),
            stderr: "branch is checked out elsewhere".into(),
        },
    );
    let repo = git.open(Path::new(ROOT));
    let outcome = run(
        repo.as_ref(),
        Path::new(LANE),
        Action::Worktree {
            expected: LaneRemovalState::Registered,
            delete_folder: false,
            force: false,
            branch: Some("feature".into()),
        },
    )
    .unwrap();
    assert!(matches!(
        outcome,
        Outcome::Removed {
            branch_error: Some(_)
        }
    ));
    assert!(
        run(
            repo.as_ref(),
            Path::new(LANE),
            Action::Branch("feature".into())
        )
        .is_err()
    );
    assert_eq!(
        git.calls()
            .iter()
            .filter(|call| matches!(call, GitCall::RemoveLane { .. }))
            .count(),
        1
    );
    assert_eq!(
        git.calls()
            .iter()
            .filter(|call| matches!(call, GitCall::DeleteBranch { .. }))
            .count(),
        2
    );
}

#[test]
fn failed_inspection_never_removes_a_lane_or_branch() {
    let git = FakeGit::new();
    git.add_repo(FakeRepo::at(ROOT));
    git.fail(ROOT, GitOp::LaneRemovalState, FakeFailure::NotFound);
    assert!(
        run(
            git.open(Path::new(ROOT)).as_ref(),
            Path::new(LANE),
            Action::Worktree {
                expected: LaneRemovalState::Missing,
                delete_folder: false,
                force: false,
                branch: Some("feature".into()),
            }
        )
        .is_err()
    );
    assert!(git.calls().is_empty());
}
