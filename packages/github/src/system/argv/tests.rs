use super::*;

/// The argv of each mutation; none is ever run from a test.
#[test]
fn mutations_build_their_gh_argv() {
    let repo = RepoId::new("GHE.corp", "O", "r");
    let sha = "0123abcdef";
    let merge = |method| merge_args(&repo, 7, method, sha).unwrap();
    let names = |names: &[&str]| names.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>();
    let reviewers = |add: &[&str], remove: &[&str]| {
        edit_reviewers_args(&repo, 7, &names(add), &names(remove)).unwrap()
    };
    let new = NewPr {
        head: "feat".into(),
        base: Some("main".into()),
        title: "Add it".into(),
        body: "a long description".into(),
        draft: true,
        worktree: std::path::PathBuf::new(),
    };
    let cases: [(&str, Vec<String>, &[&str]); 14] = [
        (
            "create, its description on stdin",
            create_args(&repo, &new),
            &[
                "pr",
                "create",
                "--repo",
                "ghe.corp/o/r",
                "--head",
                "feat",
                "--title",
                "Add it",
                "--body-file",
                "-",
                "--base",
                "main",
                "--draft",
            ],
        ),
        (
            "squash",
            merge(MergeMethod::Squash),
            &[
                "pr",
                "merge",
                "7",
                "--repo",
                "ghe.corp/o/r",
                "--squash",
                "--match-head-commit=0123abcdef",
            ],
        ),
        (
            "merge commit",
            merge(MergeMethod::Merge),
            &[
                "pr",
                "merge",
                "7",
                "--repo",
                "ghe.corp/o/r",
                "--merge",
                "--match-head-commit=0123abcdef",
            ],
        ),
        (
            "rebase",
            merge(MergeMethod::Rebase),
            &[
                "pr",
                "merge",
                "7",
                "--repo",
                "ghe.corp/o/r",
                "--rebase",
                "--match-head-commit=0123abcdef",
            ],
        ),
        (
            "ready",
            ready_args(&repo, 7),
            &["pr", "ready", "7", "--repo", "ghe.corp/o/r"],
        ),
        (
            "close",
            close_args(&repo, 7),
            &["pr", "close", "7", "--repo", "ghe.corp/o/r"],
        ),
        (
            "reopen",
            reopen_args(&repo, 7),
            &["pr", "reopen", "7", "--repo", "ghe.corp/o/r"],
        ),
        (
            "values stay strings",
            Graphql::on(&repo, "M")
                .string("pr", "@x")
                .string("head", "123")
                .args(),
            &[
                "api",
                "graphql",
                "--hostname",
                "ghe.corp",
                "-f",
                "query=M",
                "-f",
                "pr=@x",
                "-f",
                "head=123",
            ],
        ),
        (
            "an Int and a Boolean are typed, a string never",
            Graphql::on(&repo, "M")
                .pr(&repo, 7)
                .boolean("stack", true)
                .string("branch", "42")
                .args(),
            &[
                "api",
                "graphql",
                "--hostname",
                "ghe.corp",
                "-f",
                "query=M",
                "-f",
                "owner=o",
                "-f",
                "name=r",
                "-F",
                "number=7",
                "-F",
                "stack=true",
                "-f",
                "branch=42",
            ],
        ),
        (
            "turn auto-merge off",
            Graphql::on(&repo, "M").string("pr", "PR_1").args(),
            &[
                "api",
                "graphql",
                "--hostname",
                "ghe.corp",
                "-f",
                "query=M",
                "-f",
                "pr=PR_1",
            ],
        ),
        (
            "rerun failed",
            rerun_failed_args(&repo, 991),
            &["run", "rerun", "991", "--failed", "--repo", "ghe.corp/o/r"],
        ),
        (
            "ask reviewers",
            reviewers(&["al_ice", "org/core"], &[]),
            &[
                "pr",
                "edit",
                "7",
                "--repo",
                "ghe.corp/o/r",
                "--add-reviewer=al_ice,org/core",
            ],
        ),
        (
            "unask a reviewer",
            reviewers(&[], &["bob"]),
            &[
                "pr",
                "edit",
                "7",
                "--repo",
                "ghe.corp/o/r",
                "--remove-reviewer=bob",
            ],
        ),
        (
            "both",
            reviewers(&["a"], &["b"]),
            &[
                "pr",
                "edit",
                "7",
                "--repo",
                "ghe.corp/o/r",
                "--add-reviewer=a",
                "--remove-reviewer=b",
            ],
        ),
    ];
    for (label, args, expected) in cases {
        assert_eq!(args, expected, "{label}");
        for never in ["--delete-branch", "-d", "--auto", "--admin"] {
            assert!(!args.iter().any(|a| a == never), "{label}: {never}");
        }
    }
}
