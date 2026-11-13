use std::path::{Path, PathBuf};
use std::time::Duration;

use super::*;
use crate::lane::git::base::{BaseProblem, BaseTips, ResolvedBase};
use crate::lane::git::{GitFileEntry, MergeOutcome};

const ROOT: &str = "/fake/repo";

fn exit_code(error: GitError) -> Option<i32> {
    match error {
        GitError::Exit { code, .. } => code,
        other => panic!("expected an exit, got {other:?}"),
    }
}

fn dirty() -> GitWorktreeStatus {
    GitWorktreeStatus {
        unstaged: vec![GitFileEntry {
            x: ' ',
            y: 'M',
            path: PathBuf::from("a.txt"),
            original_path: None,
        }],
        ..GitWorktreeStatus::default()
    }
}

#[test]
fn a_plain_directory_answers_like_git_outside_a_repository() {
    let git = FakeGit::new();
    let dir = Path::new("/fake/plain");
    assert!(git.available());
    assert!(git.probe(dir).is_none());
    let repo = git.open(dir);
    assert_eq!(exit_code(repo.worktree_status().unwrap_err()), Some(128));
    assert_eq!(
        exit_code(repo.diff(Path::new("a"), false).unwrap_err()),
        Some(128)
    );
    assert_eq!(repo.default_branch(), None);
    assert!(matches!(
        repo.base_tips(Some("main"), None),
        Err(BaseProblem::Git(_))
    ));
    assert_eq!(
        repo.base_tips(None, None),
        Err(BaseProblem::NoBaseConfigured)
    );
    // A write that cannot land is still the code under test asking for it.
    assert_eq!(exit_code(repo.commit("m").unwrap_err()), Some(128));
    assert_eq!(
        git.calls(),
        vec![GitCall::Commit {
            repo: dir.to_path_buf(),
            message: "m".into()
        }]
    );
}

#[test]
fn a_registered_repository_answers_from_its_script() {
    let tips = BaseTips {
        base: ResolvedBase {
            label: "origin/main".into(),
            sha: "b".into(),
        },
        head: "h".into(),
    };
    let git = FakeGit::new().with(
        FakeRepo::at(ROOT)
            .branch("feature")
            .status(dirty())
            .diff("a.txt", false, "@@ diff")
            .staged_blob("a.txt", b"staged".to_vec())
            .head_message("tip")
            .base_tips(Ok(tips.clone()))
            .show_at("HEAD", "a.txt", b"old".to_vec())
            .merge_outcome(MergeOutcome::Conflicts(vec!["a.txt".into()])),
    );
    let repo = git.open(Path::new(ROOT));
    assert_eq!(repo.worktree_status().unwrap(), dirty());
    assert_eq!(repo.tracking().unwrap().branch.as_deref(), Some("feature"));
    assert_eq!(repo.default_branch().as_deref(), Some("feature"));
    assert_eq!(repo.diff(Path::new("a.txt"), false).unwrap(), "@@ diff");
    assert_eq!(repo.diff(Path::new("a.txt"), true).unwrap(), "");
    assert_eq!(repo.show_staged(Path::new("a.txt")).unwrap(), b"staged");
    assert_eq!(
        exit_code(repo.show_staged(Path::new("b")).unwrap_err()),
        Some(128)
    );
    assert_eq!(repo.head_message().unwrap(), "tip");
    assert_eq!(repo.base_tips(Some("main"), None).unwrap(), tips);
    assert_eq!(repo.changes_since(tips.clone()).unwrap().merge_base, "b");
    assert_eq!(repo.show_at("HEAD", Path::new("a.txt")).unwrap(), b"old");
    assert_eq!(
        repo.merge("other").unwrap(),
        MergeOutcome::Conflicts(vec!["a.txt".into()])
    );
    assert_eq!(repo.dirs().unwrap().git_dir, Path::new(ROOT).join(".git"));
    let probe = git.probe(Path::new(ROOT)).unwrap();
    assert_eq!(probe.repo_root, Path::new(ROOT));
    assert_eq!(probe.lanes.len(), 1);
    assert_eq!(probe.lanes[0].branch.as_deref(), Some("feature"));
}

#[test]
fn a_subpath_routes_to_the_deepest_enclosing_repository() {
    let lane = format!("{ROOT}/.claude/worktrees/lane");
    let git = FakeGit::new()
        .with(FakeRepo::at(ROOT).branch("main"))
        .with(FakeRepo::at(&lane).branch("lane"));
    let in_main = git.open(&Path::new(ROOT).join("src"));
    let in_lane = git.open(&Path::new(&lane).join("src"));
    assert_eq!(in_main.default_branch().as_deref(), Some("main"));
    assert_eq!(in_lane.default_branch().as_deref(), Some("lane"));
    assert_eq!(
        git.probe(&Path::new(&lane).join("src")).unwrap().repo_root,
        Path::new(&lane)
    );
}

#[test]
fn writes_are_logged_in_order_and_never_applied() {
    let git = FakeGit::new().with(FakeRepo::at(ROOT));
    let repo = git.open(Path::new(ROOT));
    repo.add(Path::new("a.txt")).unwrap();
    repo.commit("first").unwrap();
    repo.add_lane(Path::new("/fake/lane"), Some("feat"), Some("main"))
        .unwrap();
    git.init(Path::new("/fake/new")).unwrap();
    let root = PathBuf::from(ROOT);
    assert_eq!(
        git.calls(),
        vec![
            GitCall::Add {
                repo: root.clone(),
                path: "a.txt".into()
            },
            GitCall::Commit {
                repo: root.clone(),
                message: "first".into()
            },
            GitCall::AddLane {
                repo: root,
                new_path: "/fake/lane".into(),
                new_branch: Some("feat".into()),
                base: Some("main".into()),
            },
            GitCall::Init {
                path: "/fake/new".into()
            },
        ]
    );
    // Nothing was staged or committed: the status is still the script's.
    assert_eq!(
        repo.worktree_status().unwrap(),
        GitWorktreeStatus::default()
    );
    assert!(git.probe(Path::new("/fake/new")).is_none());
}

#[test]
fn a_branch_exists_when_checked_out_or_scripted() {
    let git = FakeGit::new().with(FakeRepo::at(ROOT).branch("main").local_branches(["feat"]));
    let repo = git.open(Path::new(ROOT));
    // (label, branch, exists)
    for (label, branch, exists) in [
        ("checked out", "main", true),
        ("scripted", "feat", true),
        ("neither", "other", false),
    ] {
        assert_eq!(repo.has_local_branch(branch).unwrap(), exists, "{label}");
    }
    assert_eq!(git.reads(GitOp::HasLocalBranch, Path::new(ROOT)), 3);
    assert_eq!(
        exit_code(
            git.open(Path::new("/fake/plain"))
                .has_local_branch("main")
                .unwrap_err()
        ),
        Some(128)
    );
}

#[test]
fn a_ref_fetch_is_logged_with_its_remote_and_refspec() {
    let git = FakeGit::new().with(FakeRepo::at(ROOT));
    git.open(Path::new(ROOT))
        .fetch_ref("origin", "+a:b")
        .unwrap();
    assert_eq!(
        git.calls(),
        vec![GitCall::FetchRef {
            repo: ROOT.into(),
            remote: "origin".into(),
            refspec: "+a:b".into(),
        }]
    );
}

#[test]
fn reads_are_counted_per_op_and_handle_path() {
    let git = FakeGit::new().with(FakeRepo::at(ROOT));
    let sub = Path::new(ROOT).join("src");
    let _ = git.open(Path::new(ROOT)).worktree_status();
    let _ = git.open(Path::new(ROOT)).worktree_status();
    let _ = git.open(&sub).worktree_status();
    assert_eq!(git.reads(GitOp::WorktreeStatus, Path::new(ROOT)), 2);
    assert_eq!(git.reads(GitOp::WorktreeStatus, &sub), 1);
    assert_eq!(git.reads(GitOp::Open, Path::new(ROOT)), 2);
    assert_eq!(git.reads(GitOp::Tracking, Path::new(ROOT)), 0);
}

#[test]
fn an_injected_failure_hits_only_its_path_and_op() {
    let git = FakeGit::new().with(FakeRepo::at(ROOT));
    git.fail(
        ROOT,
        GitOp::Push,
        FakeFailure::Timeout(Duration::from_secs(60)),
    );
    git.fail(
        ROOT,
        GitOp::BaseTips,
        FakeFailure::Exit {
            code: Some(1),
            stderr: "boom".into(),
        },
    );
    let repo = git.open(Path::new(ROOT));
    assert!(matches!(repo.push(), Err(GitError::Timeout(_))));
    assert!(matches!(repo.push(), Err(GitError::Timeout(_))));
    assert!(repo.fetch().is_ok());
    assert!(git.open(&Path::new(ROOT).join("src")).push().is_ok());
    assert!(matches!(
        repo.base_tips(Some("main"), None),
        Err(BaseProblem::Git(_))
    ));
}

#[test]
fn set_status_changes_later_reads() {
    let git = FakeGit::new().with(FakeRepo::at(ROOT).status(dirty()));
    let repo = git.open(Path::new(ROOT));
    assert_eq!(repo.worktree_status().unwrap(), dirty());
    git.set_status(Path::new(ROOT), GitWorktreeStatus::default());
    assert_eq!(
        repo.worktree_status().unwrap(),
        GitWorktreeStatus::default()
    );
    git.set_tracking(
        Path::new(ROOT),
        GitTracking {
            ahead: 1,
            ..GitTracking::default()
        },
    );
    assert_eq!(repo.tracking().unwrap().ahead, 1);
}

#[test]
fn missing_git_fails_every_command_with_not_found() {
    let git = FakeGit::new().with(FakeRepo::at(ROOT));
    git.set_available(false);
    assert!(!git.available());
    assert!(git.probe(Path::new(ROOT)).is_none());
    assert!(matches!(git.init(Path::new(ROOT)), Err(GitError::NotFound)));
    let repo = git.open(Path::new(ROOT));
    assert!(matches!(repo.worktree_status(), Err(GitError::NotFound)));
    assert!(matches!(repo.commit("m"), Err(GitError::NotFound)));
    assert_eq!(repo.default_branch(), None);
}

#[test]
fn a_lane_on_its_scripted_base_branch_has_no_comparison() {
    let tips = BaseTips {
        base: ResolvedBase {
            label: "origin/main".into(),
            sha: "b".into(),
        },
        head: "h".into(),
    };
    let git = FakeGit::new().with(FakeRepo::at(ROOT).base_tips(Ok(tips.clone())));
    let repo = git.open(Path::new(ROOT));
    for current in ["main", "origin/main"] {
        assert_eq!(
            repo.base_tips(Some("main"), Some(current)),
            Err(BaseProblem::OnBaseBranch),
            "{current}"
        );
    }
    assert_eq!(repo.base_tips(Some("main"), Some("feature")), Ok(tips));
    // HEAD is read after the branch check, so an unborn lane on its base is
    // still on its base.
    let git = FakeGit::new().with(FakeRepo::at(ROOT).base_tips(Err(BaseProblem::UnbornHead)));
    let repo = git.open(Path::new(ROOT));
    assert_eq!(
        repo.base_tips(Some("main"), Some("main")),
        Err(BaseProblem::OnBaseBranch)
    );
    assert_eq!(
        repo.base_tips(Some("main"), Some("feature")),
        Err(BaseProblem::UnbornHead)
    );
    // A base that resolves to nothing is reported before the branch check.
    let git = FakeGit::new().with(FakeRepo::at(ROOT));
    assert_eq!(
        git.open(Path::new(ROOT))
            .base_tips(Some("main"), Some("main")),
        Err(BaseProblem::NotFound("main".into()))
    );
}

#[test]
fn a_scripted_push_target_is_answered_and_an_upstream_push_logged() {
    let target = crate::lane::git::PushTarget {
        remote: "origin".into(),
        url: "git@github.com:o/r.git".into(),
        branch: "feat".into(),
        published: false,
        has_upstream: false,
    };
    let git = FakeGit::new()
        .with(FakeRepo::at(ROOT).branch("feat"))
        .with(FakeRepo::at("/fake/other").push_target(target.clone()));
    assert_eq!(git.open(Path::new(ROOT)).push_target().unwrap(), None);
    let repo = git.open(Path::new("/fake/other"));
    assert_eq!(repo.push_target().unwrap(), Some(target));
    repo.push_head("origin", "feat", true).unwrap();
    assert_eq!(
        git.calls(),
        vec![GitCall::PushHead {
            repo: PathBuf::from("/fake/other"),
            remote: "origin".into(),
            branch: "feat".into(),
            set_upstream: true,
        }]
    );
}
