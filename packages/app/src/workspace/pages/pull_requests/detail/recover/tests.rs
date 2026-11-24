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
        matches!(&error.failure, RecoverFailure::Unavailable(reason) if reason.contains("gone.rs")),
        "a side the change says exists is missing: {error:?}"
    );
    assert!(error.cause.is_none());

    let unchanged = RecoverTarget {
        lines: 3,
        ..target(FileChange::Modified)
    };
    gh.set_file_at(&repo, "new.rs", "abc", FileText::Text("a\nb\n".into()));
    assert!(
        matches!(
            failed(&gh, &unchanged).failure,
            RecoverFailure::Unavailable(_)
        ),
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

#[test]
fn file_links_open_the_pr_change_for_forks_deletions_and_reserved_path_characters() {
    let pr = "https://ghe.example:8443/base/repo/pull/51/?q=test#discussion_r9";
    let cases = [
        (
            "src/index.js",
            "bfe9874d239014961b1ae4e89875a6155667db834a410aaaa2ebe3cf89820556",
        ),
        (
            "src/한 글#?%.rs",
            "2c96d9f622a801aef74c7f29246464747af734421d86e7d387b4a29b0f18b4c0",
        ),
    ];
    for (path, hash) in cases {
        assert_eq!(
            file_diff_url(pr, path),
            Some(format!(
                "https://ghe.example:8443/base/repo/pull/51/files#diff-{hash}"
            ))
        );
    }
    for url in [
        "javascript:alert(1)",
        "http://user@host/o/r/pull/5",
        "https://host/o/r",
        "https://host/o/r/pull/5/checks",
    ] {
        assert_eq!(file_diff_url(url, "file"), None, "{url}");
    }
}

#[test]
fn recovery_offers_retry_for_temporary_gh_errors() {
    let cases = [
        (GhError::NotFound, false),
        (
            GhError::Exit {
                code: Some(4),
                stderr: "login required".into(),
            },
            false,
        ),
        (GhError::Overflow { limit: 1024 }, false),
        (GhError::Timeout(std::time::Duration::from_secs(3)), true),
        (
            GhError::Exit {
                code: Some(1),
                stderr: "rate limit exceeded".into(),
            },
            true,
        ),
    ];
    for (error, retry) in cases {
        let failure = RecoverError::gh(error).failure;
        assert_eq!(failure.can_retry(), retry, "{failure:?}");
    }
}
