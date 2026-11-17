use std::path::{Path, PathBuf};

use super::*;
use crate::lane::git::base::{AgainstBase, BaseTips, RangeFile, ResolvedBase};
use crate::lane::git::{FakeGit, FakeRepo, Git as _, GitFileEntry, GitWorktreeStatus};

fn push() -> PushTarget {
    PushTarget {
        remote: "origin".into(),
        url: "git@github.com:o/r.git".into(),
        branch: "feat".into(),
        published: true,
        has_upstream: true,
        review: None,
    }
}

fn tips() -> BaseTips {
    BaseTips {
        base: ResolvedBase {
            label: "origin/main".into(),
            sha: "b".into(),
        },
        head: "h".into(),
    }
}

fn file(path: &str) -> RangeFile {
    RangeFile {
        path: PathBuf::from(path),
        old_path: None,
        status: 'M',
        added: 1,
        removed: 0,
    }
}

fn entry(path: &str, y: char) -> GitFileEntry {
    GitFileEntry {
        x: 'M',
        y,
        path: PathBuf::from(path),
        original_path: None,
    }
}

fn open(repo: FakeRepo) -> std::sync::Arc<dyn GitRepo> {
    FakeGit::new().with(repo).open(Path::new("/r"))
}

#[test]
fn the_publication_names_the_target_and_what_stays_behind() {
    let repo = open(
        FakeRepo::at("/r")
            .push_target(push())
            .status(GitWorktreeStatus {
                staged: vec![entry("a.rs", ' ')],
                // Staged and changed again: one file, not two; untracked counts.
                unstaged: vec![entry("a.rs", 'M'), entry("new.rs", '?')],
                diffstat: Default::default(),
            }),
    );
    let read = read_publication(repo.as_ref());
    assert_eq!(
        read.target,
        TargetRead::Pushes {
            repo: Some(daruda_github::RepoId::new("github.com", "o", "r")),
            shown: ShownTarget::of(&push()),
        }
    );
    assert_eq!(read.uncommitted, 2);
}

#[test]
fn a_target_that_is_not_github_or_absent_is_still_checked() {
    let mut elsewhere = push();
    elsewhere.url = "/srv/git/r.git".into();
    let read = read_publication(open(FakeRepo::at("/r").push_target(elsewhere.clone())).as_ref());
    assert_eq!(
        read.target,
        TargetRead::Pushes {
            repo: None,
            shown: ShownTarget::of(&elsewhere),
        }
    );
    let none = read_publication(open(FakeRepo::at("/r")).as_ref());
    assert_eq!(none.target, TargetRead::NoTarget);
    assert_eq!(none.target.expected(), Expected::NoTarget);
}

#[test]
fn against_says_what_it_counted_against_or_why_not() {
    let counted = open(
        FakeRepo::at("/r")
            .base_tips(Ok(tips()))
            .changes_since(Ok(AgainstBase {
                tips: tips(),
                merge_base: "b".into(),
                files: vec![file("a.rs"), file("b.rs")],
                commits: 3,
            })),
    );
    assert_eq!(
        read_against(counted.as_ref(), Some("main"), Some("feat")),
        Against::Counted {
            label: "origin/main".into(),
            commits: 3,
            files: 2,
        }
    );
    let plain = open(FakeRepo::at("/r"));
    assert_eq!(
        read_against(plain.as_ref(), Some("mian"), Some("feat")),
        Against::BaseNotFound("mian".into())
    );
    assert_eq!(
        read_against(plain.as_ref(), None, Some("feat")),
        Against::DefaultBase
    );
    let on_base = open(FakeRepo::at("/r").base_tips(Err(BaseProblem::OnBaseBranch)));
    assert_eq!(
        read_against(on_base.as_ref(), Some("feat"), Some("feat")),
        Against::OnBase
    );
}

#[test]
fn what_was_shown_must_still_hold_when_pushing() {
    let shown = Expected::Target(ShownTarget::of(&push()));
    let other = |edit: fn(&mut PushTarget)| {
        let mut push = push();
        edit(&mut push);
        push
    };
    // (label, expected, read again, holds)
    let cases = [
        ("same", shown.clone(), Some(push()), true),
        (
            "other remote",
            shown.clone(),
            Some(other(|p| p.remote = "fork".into())),
            false,
        ),
        (
            "other url",
            shown.clone(),
            Some(other(|p| p.url = "x".into())),
            false,
        ),
        (
            "other branch",
            shown.clone(),
            Some(other(|p| p.branch = "f2".into())),
            false,
        ),
        (
            "published since",
            shown.clone(),
            Some(other(|p| p.published = false)),
            true,
        ),
        ("gone", shown, None, false),
        (
            "none shown, one now",
            Expected::NoTarget,
            Some(push()),
            false,
        ),
        ("none shown, none now", Expected::NoTarget, None, true),
        ("unchecked", Expected::Unchecked, Some(push()), true),
    ];
    for (label, expected, read, holds) in cases {
        assert_eq!(expected.holds(read.as_ref()), holds, "{label}");
    }
}
