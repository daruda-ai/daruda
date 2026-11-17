use daruda_github::fake::{FakeGh, GhCall, GhFailure, GhOp};

use super::*;

fn target(change: FileChange) -> RecoverTarget {
    RecoverTarget {
        repo: RepoId::new("github.com", "o", "n"),
        base_branch: "main".into(),
        head: "abc".into(),
        merge_base: None,
        path: "new.rs".into(),
        change,
        lines: 2,
    }
}

const BASE: &str = "merge-base:main...abc";

fn scripted() -> FakeGh {
    let gh = FakeGh::new();
    let repo = target(FileChange::Modified).repo;
    gh.set_file_at(&repo, "old.rs", BASE, FileText::Text("a\nb\n".into()));
    gh.set_file_at(&repo, "new.rs", BASE, FileText::Text("a\nb\n".into()));
    gh.set_file_at(&repo, "new.rs", "abc", FileText::Text("a\nc\n".into()));
    gh
}

#[test]
fn both_sides_are_read_at_the_merge_base_and_the_head() {
    let gh = scripted();
    let from = || "old.rs".to_owned();
    // (label, change, removed lines, added lines)
    let cases = [
        ("an edit", FileChange::Modified, vec!["-b"], vec!["+c"]),
        (
            "a rename reads the old side under its old name",
            FileChange::Renamed { from: from() },
            vec!["-b"],
            vec!["+c"],
        ),
        (
            "a copy too",
            FileChange::Copied { from: from() },
            vec!["-b"],
            vec!["+c"],
        ),
        (
            "an added file has no old side",
            FileChange::Added,
            vec![],
            vec!["+a", "+c"],
        ),
        (
            "a removed file has no new side",
            FileChange::Removed,
            vec!["-a", "-b"],
            vec![],
        ),
    ];
    for (label, change, removed, added) in cases {
        let read = recover_patch(&gh, &target(change)).expect(label);
        assert_eq!(read.merge_base, BASE, "{label}");
        let changed: Vec<&str> = read
            .patch
            .lines()
            .filter(|line| line.starts_with(['+', '-']))
            .collect();
        let expected: Vec<&str> = removed.into_iter().chain(added).collect();
        assert_eq!(changed, expected, "{label}");
    }
}

#[test]
fn a_merge_base_already_found_is_not_asked_for_again() {
    let gh = scripted();
    let known = RecoverTarget {
        merge_base: Some(BASE.into()),
        ..target(FileChange::Modified)
    };
    recover_patch(&gh, &known).unwrap();
    assert!(
        !gh.calls()
            .iter()
            .any(|call| matches!(call, GhCall::MergeBase { .. })),
        "{:?}",
        gh.calls()
    );
}

#[test]
fn unreadable_sides_say_why_instead_of_diffing_as_empty() {
    let failed = |gh: &FakeGh, target: &RecoverTarget| recover_patch(gh, target).unwrap_err();
    let gh = scripted();
    let repo = target(FileChange::Modified).repo;

    let missing = RecoverTarget {
        path: "gone.rs".into(),
        ..target(FileChange::Modified)
    };
    let error = failed(&gh, &missing);
    assert!(
        matches!(&error.failure, RecoverFailure::Failed(reason) if reason.contains("gone.rs")),
        "a side the change says exists is missing: {error:?}"
    );
    assert!(error.cause.is_none());

    let unchanged = RecoverTarget {
        lines: 3,
        ..target(FileChange::Modified)
    };
    gh.set_file_at(&repo, "new.rs", "abc", FileText::Text("a\nb\n".into()));
    assert!(
        matches!(failed(&gh, &unchanged).failure, RecoverFailure::Failed(_)),
        "no difference where the listing counts lines"
    );

    gh.set_file_at(&repo, "new.rs", "abc", FileText::Binary);
    assert_eq!(
        failed(&gh, &target(FileChange::Modified)).failure,
        RecoverFailure::Binary
    );

    gh.fail(
        GhOp::MergeBase,
        GhFailure::Exit {
            code: Some(1),
            stderr: "HTTP 502".into(),
        },
    );
    let error = failed(&gh, &target(FileChange::Modified));
    assert!(
        matches!(&error.failure, RecoverFailure::Failed(reason) if reason.contains("HTTP 502"))
    );
    assert!(error.cause.is_some(), "a gh failure is reported");
}
