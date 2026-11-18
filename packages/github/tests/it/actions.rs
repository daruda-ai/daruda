use daruda_github::{
    ActionsJob, BlockReason, CheckEntry, CheckKind, CheckRollup, CheckRun, CheckStatus, MergeBlock,
    MergeMethod, Mergeability, Permission, PrDetail, PrState, PrSummary, ViewerCan,
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
        viewer_can: ViewerCan::all().with(Permission::Update, false),
        auto_merge: None,
        queue: None,
        node_id: "PR_1".into(),
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

#[test]
fn close_and_reopen_follow_state_and_permission() {
    let open = Mergeability::Ready;
    // (label, state, viewer may close, viewer may reopen, can close, can reopen)
    let cases = [
        ("open", PrState::Open, true, true, true, false),
        ("draft", PrState::Draft, true, false, true, false),
        (
            "open, not the viewer's to close",
            PrState::Open,
            false,
            true,
            false,
            false,
        ),
        ("closed", PrState::Closed, true, true, false, true),
        (
            "closed, not the viewer's to reopen",
            PrState::Closed,
            true,
            false,
            false,
            false,
        ),
        (
            "merged never reopens",
            PrState::Merged,
            true,
            true,
            false,
            false,
        ),
    ];
    for (label, state, may_close, may_reopen, close, reopen) in cases {
        let mut pr = detail(state, open, Vec::new());
        pr.viewer_can = ViewerCan::default()
            .with(Permission::Close, may_close)
            .with(Permission::Reopen, may_reopen);
        assert_eq!(pr.can_close_now(), close, "{label}: close");
        assert_eq!(pr.can_reopen_now(), reopen, "{label}: reopen");
    }
}

#[test]
fn auto_merge_is_offered_only_where_github_would_take_it() {
    use daruda_github::{AutoMergeOn, QueueEntry, QueueState};
    let base = || PrDetail {
        viewer_can: ViewerCan::all(),
        ..detail(
            PrState::Open,
            Mergeability::Blocked(BlockReason::Requirements),
            vec![MergeMethod::Squash, MergeMethod::Merge],
        )
    };
    let on = Some(AutoMergeOn {
        method: Some(MergeMethod::Squash),
        by: None,
    });
    let queued = Some(QueueEntry {
        position: Some(1),
        state: QueueState::Queued,
    });
    // (label, pull request, offered on, can turn off)
    let cases = [
        ("waiting on requirements", base(), true, false),
        (
            "not permitted",
            PrDetail {
                viewer_can: ViewerCan::all().with(Permission::EnableAutoMerge, false),
                ..base()
            },
            false,
            false,
        ),
        (
            "a draft",
            PrDetail {
                summary: PrSummary {
                    state: PrState::Draft,
                    ..base().summary
                },
                ..base()
            },
            false,
            false,
        ),
        (
            "conflicting",
            PrDetail {
                mergeability: Mergeability::Conflicting,
                ..base()
            },
            false,
            false,
        ),
        (
            "ready to merge now",
            PrDetail {
                mergeability: Mergeability::Ready,
                ..base()
            },
            false,
            false,
        ),
        (
            "behind its base",
            PrDetail {
                mergeability: Mergeability::Blocked(BlockReason::Behind),
                ..base()
            },
            true,
            false,
        ),
        (
            "already on",
            PrDetail {
                auto_merge: on.clone(),
                ..base()
            },
            false,
            true,
        ),
        (
            "on, not the viewer's to turn off",
            PrDetail {
                auto_merge: on.clone(),
                viewer_can: ViewerCan::all().with(Permission::DisableAutoMerge, false),
                ..base()
            },
            false,
            false,
        ),
        (
            "queued",
            PrDetail {
                queue: queued,
                ..base()
            },
            false,
            false,
        ),
    ];
    for (label, pr, offered, off) in cases {
        assert_eq!(
            !pr.auto_merge_menu().is_empty(),
            offered,
            "{label}: offered"
        );
        assert_eq!(
            pr.can_enable_auto_merge_with(MergeMethod::Squash),
            offered,
            "{label}: squash"
        );
        assert!(
            !pr.can_enable_auto_merge_with(MergeMethod::Rebase),
            "{label}: not allowed"
        );
        assert_eq!(pr.can_disable_auto_merge_now(), off, "{label}: off");
    }
    let queued = PrDetail {
        queue: queued,
        mergeability: Mergeability::Ready,
        ..base()
    };
    assert_eq!(
        queued.merge_block(),
        Some(MergeBlock::Queued),
        "no second merge"
    );
    let on_and_ready = PrDetail {
        auto_merge: on,
        mergeability: Mergeability::Ready,
        ..base()
    };
    assert_eq!(
        on_and_ready.merge_block(),
        None,
        "merging now stays possible"
    );
}

/// A pull request in the merge queue, with or without auto-merge, can be
/// taken out while open; one closed or merged since has left it.
#[test]
fn a_queued_pull_request_can_be_taken_out_of_the_queue() {
    use daruda_github::{QueueEntry, QueueState};
    let queued = |state: PrState| PrDetail {
        viewer_can: ViewerCan::all(),
        queue: Some(QueueEntry {
            position: Some(2),
            state: QueueState::AwaitingChecks,
        }),
        ..detail(state, Mergeability::Ready, vec![MergeMethod::Squash])
    };
    let unqueued = PrDetail {
        queue: None,
        ..queued(PrState::Open)
    };
    // (label, pull request, can take out)
    let cases = [
        ("queued, no auto-merge", queued(PrState::Open), true),
        ("not queued", unqueued, false),
        ("closed since", queued(PrState::Closed), false),
        ("merged since", queued(PrState::Merged), false),
    ];
    for (label, pr, out) in cases {
        assert_eq!(pr.can_dequeue_now(), out, "{label}");
        assert!(!pr.can_disable_auto_merge_now(), "{label}: no auto-merge");
    }
}
