use daruda_github::{
    ActionsJob, BlockReason, CheckEntry, CheckKind, CheckRollup, CheckRun, CheckStatus, MergeBlock,
    MergeMethod, Mergeability, PrDetail, PrState, PrSummary,
};

fn detail(state: PrState, mergeability: Mergeability, methods: Vec<MergeMethod>) -> PrDetail {
    PrDetail {
        summary: PrSummary {
            number: 1,
            url: String::new(),
            title: String::new(),
            state,
            head_branch: "h".into(),
            cross_repo: false,
            checks: CheckRollup::None,
            failed: Vec::new(),
            review: None,
            updated_at: String::new(),
        },
        body: String::new(),
        head_sha: "abc".into(),
        base_branch: "main".into(),
        author: None,
        mergeability,
        methods,
        default_method: None,
        reviewers: Vec::new(),
        checks: Vec::new(),
        checks_truncated: false,
        open_threads: 0,
        open_threads_more: false,
        can_update: false,
        node_id: None,
        changed_files: None,
    }
}

#[test]
fn the_menu_offers_only_allowed_methods_default_first() {
    use MergeMethod::{Merge, Rebase, Squash};
    /// (label, allowed as GitHub lists them, GitHub's default, menu)
    type Case = (
        &'static str,
        Vec<MergeMethod>,
        Option<MergeMethod>,
        Vec<MergeMethod>,
    );
    let cases: [Case; 8] = [
        (
            "the default leads",
            vec![Merge, Squash, Rebase],
            Some(Rebase),
            vec![Rebase, Squash, Merge],
        ),
        (
            "merge as default",
            vec![Merge, Squash, Rebase],
            Some(Merge),
            vec![Merge, Squash, Rebase],
        ),
        (
            "a default no longer allowed falls back",
            vec![Merge, Rebase],
            Some(Squash),
            vec![Merge, Rebase],
        ),
        (
            "no default: the fixed order",
            vec![Merge, Squash, Rebase],
            None,
            vec![Squash, Merge, Rebase],
        ),
        ("no squash", vec![Rebase, Merge], None, vec![Merge, Rebase]),
        ("rebase only", vec![Rebase], Some(Rebase), vec![Rebase]),
        (
            "squash and rebase",
            vec![Rebase, Squash],
            None,
            vec![Squash, Rebase],
        ),
        ("none", vec![], Some(Merge), vec![]),
    ];
    for (label, allowed, default, menu) in cases {
        let loaded = PrDetail {
            default_method: default,
            ..detail(PrState::Open, Mergeability::Ready, allowed)
        };
        assert_eq!(loaded.merge_menu(), menu, "{label}");
    }
}

#[test]
fn merge_is_off_only_for_a_draft_a_conflict_or_no_method() {
    let squash = || vec![MergeMethod::Squash];
    let blocked = Mergeability::Blocked(BlockReason::Requirements);
    let draft = Mergeability::Blocked(BlockReason::Draft);
    // (label, state, mergeability, methods, block)
    let cases = [
        ("ready", PrState::Open, Mergeability::Ready, squash(), None),
        (
            "GitHub may refuse: it says so",
            PrState::Open,
            blocked,
            squash(),
            None,
        ),
        (
            "not worked out yet",
            PrState::Open,
            Mergeability::Unknown,
            squash(),
            None,
        ),
        (
            "a conflict",
            PrState::Open,
            Mergeability::Conflicting,
            squash(),
            Some(MergeBlock::Conflicting),
        ),
        (
            "a draft",
            PrState::Draft,
            blocked,
            squash(),
            Some(MergeBlock::Draft),
        ),
        (
            "a draft only by why it cannot merge",
            PrState::Open,
            draft,
            squash(),
            Some(MergeBlock::Draft),
        ),
        (
            "a draft only by its own flag",
            PrState::Draft,
            Mergeability::Unknown,
            squash(),
            Some(MergeBlock::Draft),
        ),
        (
            "no method",
            PrState::Open,
            Mergeability::Ready,
            vec![],
            Some(MergeBlock::NoMethods),
        ),
    ];
    for (label, state, mergeability, methods, block) in cases {
        let loaded = detail(state, mergeability, methods);
        assert_eq!(loaded.merge_block(), block, "{label}");
        assert_eq!(
            loaded.is_draft(),
            block == Some(MergeBlock::Draft),
            "{label}"
        );
        assert_eq!(
            loaded.can_merge(MergeMethod::Squash),
            block.is_none(),
            "{label}"
        );
    }
    let merged = detail(PrState::Merged, draft, squash());
    assert!(!merged.is_draft(), "a merged pull request is no draft");
    let ready = detail(PrState::Open, Mergeability::Ready, squash());
    assert!(
        !ready.can_merge(MergeMethod::Rebase),
        "a method not allowed"
    );
}

fn entry(status: CheckStatus, job: Option<ActionsJob>) -> CheckEntry {
    CheckEntry {
        run: CheckRun {
            kind: CheckKind::Run,
            workflow: None,
            event: None,
            name: "c".into(),
            status,
            details_url: None,
            started_at: None,
            completed_at: None,
        },
        job,
        duration: None,
    }
}

#[test]
fn each_failed_run_is_rerun_once() {
    let job = |run, job| Some(ActionsJob { run, job });
    let checks = vec![
        entry(CheckStatus::Failure, job(9, 1)),
        entry(CheckStatus::Failure, job(9, 2)),
        entry(CheckStatus::Failure, job(4, 3)),
        entry(CheckStatus::Failure, None),
        entry(CheckStatus::Success, job(5, 4)),
        entry(CheckStatus::Pending, job(6, 5)),
    ];
    let with = |checks: Vec<CheckEntry>| PrDetail {
        checks,
        ..detail(PrState::Open, Mergeability::Ready, vec![])
    };
    assert_eq!(with(checks.clone()).failed_runs(), [4, 9]);
    assert!(with(checks[3..].to_vec()).failed_runs().is_empty());
}
