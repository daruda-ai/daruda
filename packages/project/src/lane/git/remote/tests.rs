use super::*;
use crate::lane::git::tests::{commit_initial, init, require_git, teardown, unique_tmpdir};

#[test]
fn config_records_split_into_keys_and_values() {
    let raw = "remote.origin.url\nhttps://h/o/r\0core.bare\0branch.Main.merge\nrefs/heads/main\0";
    assert_eq!(
        parse_config_list(raw),
        vec![
            ("remote.origin.url", "https://h/o/r"),
            ("core.bare", ""),
            ("branch.Main.merge", "refs/heads/main"),
        ]
    );
}

/// `(label, config entries, expected (remote, remote branch))`.
type Case = (
    &'static str,
    Vec<(&'static str, &'static str)>,
    Option<(&'static str, &'static str)>,
);

#[test]
fn the_push_remote_and_branch_follow_gits_precedence() {
    let origin = ("remote.origin.url", "u");
    let fork = ("remote.fork.url", "u");
    let up = ("remote.up.url", "u");
    let cases: Vec<Case> = vec![
        ("no remote", vec![], None),
        ("only origin", vec![origin], Some(("origin", "feat"))),
        ("only another remote", vec![up], Some(("up", "feat"))),
        ("two remotes, none chosen", vec![up, fork], None),
        (
            "an upstream named differently pushes to the same name by default",
            vec![
                origin,
                up,
                ("branch.feat.remote", "up"),
                ("branch.feat.merge", "refs/heads/main"),
            ],
            Some(("up", "feat")),
        ),
        (
            "push.default=upstream follows the upstream's name",
            vec![
                origin,
                up,
                ("push.default", "upstream"),
                ("branch.feat.remote", "up"),
                ("branch.feat.merge", "refs/heads/other"),
            ],
            Some(("up", "other")),
        ),
        (
            "push.default=current keeps the local name",
            vec![
                origin,
                ("push.default", "current"),
                ("branch.feat.remote", "origin"),
                ("branch.feat.merge", "refs/heads/main"),
            ],
            Some(("origin", "feat")),
        ),
        (
            "a push remote given as a URL cannot be read",
            vec![origin, ("branch.feat.pushremote", "git@github.com:o/r.git")],
            None,
        ),
        (
            "pushDefault beats the upstream remote, keeping the local name",
            vec![
                origin,
                fork,
                ("push.default", "upstream"),
                ("remote.pushdefault", "fork"),
                ("branch.feat.remote", "origin"),
                ("branch.feat.merge", "refs/heads/other"),
            ],
            Some(("fork", "feat")),
        ),
        (
            "pushRemote beats pushDefault",
            vec![
                origin,
                fork,
                up,
                ("remote.pushdefault", "up"),
                ("branch.feat.pushremote", "fork"),
            ],
            Some(("fork", "feat")),
        ),
        (
            "a local upstream is not a remote",
            vec![origin, ("branch.feat.remote", ".")],
            Some(("origin", "feat")),
        ),
        (
            "the last value of a key wins",
            vec![
                origin,
                fork,
                ("remote.pushdefault", "origin"),
                ("remote.pushdefault", "fork"),
            ],
            Some(("fork", "feat")),
        ),
        (
            "two url lines are one remote",
            vec![up, ("remote.up.url", "v")],
            Some(("up", "feat")),
        ),
    ];
    for (label, entries, expected) in cases {
        let got = resolve("feat", &entries);
        let expected = expected.map(|(r, b)| (r.to_owned(), b.to_owned()));
        assert_eq!(got, expected, "{label}");
    }
}

#[test]
fn push_target_reads_a_real_repository() {
    if !require_git() {
        return;
    }
    let root = unique_tmpdir("push_target");
    let remote = root.join("remote.git");
    let work = root.join("work");
    init(&work).unwrap();
    commit_initial(&work);
    run_git(&work, ["checkout", "-b", "feat"]).unwrap();
    // No remote yet: nothing to push to.
    assert_eq!(push_target(&work).unwrap(), None);

    std::fs::create_dir_all(&remote).unwrap();
    run_git(&remote, ["init", "--bare", "--quiet"]).unwrap();
    let url = remote.to_str().unwrap();
    run_git(&work, ["remote", "add", "origin", url]).unwrap();
    let before = push_target(&work).unwrap().unwrap();
    assert_eq!(
        (
            before.remote.as_str(),
            before.branch.as_str(),
            before.published
        ),
        ("origin", "feat", false)
    );
    assert_eq!(before.url, url);

    push_head(&work, "origin", "feat", true).unwrap();
    let after = push_target(&work).unwrap().unwrap();
    assert!(after.published, "pushed with -u");

    // A tag named like the branch does not confuse the branch name.
    run_git(&work, ["tag", "feat"]).unwrap();
    assert_eq!(push_target(&work).unwrap().unwrap().branch, "feat");
    // Pushing again with nothing new is a no-op, not an error.
    push_head(&work, "origin", "feat", false).unwrap();

    // Detached HEAD has no branch to push.
    run_git(&work, ["checkout", "--detach"]).unwrap();
    assert_eq!(push_target(&work).unwrap(), None);
    teardown(&root);
}
