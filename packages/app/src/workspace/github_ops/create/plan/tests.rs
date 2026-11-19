use super::*;

#[test]
fn a_lane_base_becomes_a_branch_gh_can_target() {
    let cases = [
        ("origin/main", Some("main")),
        ("main", Some("main")),
        ("feature/x", Some("feature/x")),
        ("upstream/main", Some("upstream/main")),
        ("1a2b3c4d", None),
        ("origin/", None),
    ];
    for (base, expected) in cases {
        assert_eq!(base_branch(base, "origin").as_deref(), expected, "{base}");
    }
}

fn push(branch: &str, published: bool, has_upstream: bool) -> PushTarget {
    PushTarget {
        remote: "origin".into(),
        url: "git@github.com:o/r.git".into(),
        branch: branch.into(),
        published,
        has_upstream,
        review: None,
    }
}

fn request(base: Option<&str>) -> CreatePrRequest {
    CreatePrRequest {
        title: "t".into(),
        body: "b".into(),
        base: base.map(str::to_owned),
        draft: false,
        expected: crate::workspace::github_ops::create_modal::summary::Expected::Unchecked,
        stack_on: None,
    }
}

#[test]
fn a_plan_targets_the_bare_base_and_refuses_the_head_as_base() {
    // (label, head, typed base, default branch, planned base or None if refused)
    type Row = (
        &'static str,
        &'static str,
        Option<&'static str>,
        Option<&'static str>,
        Option<Option<&'static str>>,
    );
    let cases: [Row; 6] = [
        ("typed base", "feat", Some("main"), None, Some(Some("main"))),
        (
            "prefix stripped",
            "feat",
            Some("origin/main"),
            None,
            Some(Some("main")),
        ),
        (
            "other remote kept",
            "feat",
            Some("upstream/main"),
            None,
            Some(Some("upstream/main")),
        ),
        ("GitHub's choice", "feat", None, Some("main"), Some(None)),
        (
            "typed base is the head",
            "feat",
            Some("origin/feat"),
            None,
            None,
        ),
        (
            "default branch is the head",
            "main",
            None,
            Some("main"),
            None,
        ),
    ];
    for (label, head, base, default, expected) in cases {
        let planned = plan_pr(
            push(head, true, true),
            request(base),
            default,
            PathBuf::new(),
        );
        match expected {
            Some(base) => {
                let plan = planned.unwrap_or_else(|e| panic!("{label}: {e}"));
                assert_eq!(plan.new.base.as_deref(), base, "{label}");
                assert_eq!(plan.new.head, head, "{label}");
            }
            None => assert!(
                matches!(planned, Err(CreatePrError::HeadIsBase(ref b)) if b == head),
                "{label}: {planned:?}"
            ),
        }
    }
}

#[test]
fn upstream_is_set_only_for_a_branch_without_one() {
    let cases = [
        ("first push", false, false, true),
        ("already pushed", true, false, false),
        ("tracks another repository", false, true, false),
    ];
    for (label, published, has_upstream, expected) in cases {
        let plan = plan_pr(
            push("feat", published, has_upstream),
            request(None),
            None,
            PathBuf::new(),
        )
        .unwrap();
        assert_eq!(plan.set_upstream, expected, "{label}");
    }
}

#[test]
fn a_review_branch_is_never_pushed_for_a_pull_request() {
    let target = PushTarget {
        review: Some("github.com/o/r#9".into()),
        ..push("pr-9", false, false)
    };
    assert!(matches!(
        plan_pr(target, request(None), None, PathBuf::new()),
        Err(CreatePrError::ReviewBranch)
    ));
}

#[test]
fn a_remote_that_is_not_github_has_no_plan() {
    let mut target = push("feat", true, true);
    target.url = "/srv/git/r.git".into();
    assert!(matches!(
        plan_pr(target, request(None), None, PathBuf::new()),
        Err(CreatePrError::NoTarget)
    ));
}
