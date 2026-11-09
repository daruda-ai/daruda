use crate::lane::git::{FakeRepo, GitDirs, GitError};
use crate::workspace::Workspace;
use crate::workspace::left_dock::git_ops::health::{COOLDOWN, ReadOrigin};
use crate::workspace::left_dock::git_ops::status::ReadRequest;
use daruda_store::observability::error_report::ErrorReport;
use daruda_store::project::LaneRef;
use gpui::{Context, TestAppContext};
use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

fn no_repeat(_: &mut Workspace, _: LaneRef, _: &mut Context<Workspace>) {}

/// Install a fake git that answers for a repository at `dir`. Its git dir is
/// a plain directory inside the checkout, so the watcher a recovered dirs
/// probe attaches has something real to watch.
fn install_fake_repo(cx: &mut TestAppContext, dir: &Path) {
    let root = daruda_core::path::canonicalize(dir).unwrap();
    let git_dir = root.join("git-dir");
    std::fs::create_dir(&git_dir).unwrap();
    crate::test_support::fake_git(cx).add_repo(FakeRepo::at(&root).branch("main").dirs(GitDirs {
        git_dir: git_dir.clone(),
        common_dir: git_dir,
    }));
}

#[gpui::test]
fn git_refresh_health_manual_request_waits_for_another_lanes_failed_probe(cx: &mut TestAppContext) {
    use crate::lane::git::GitEnvironmentFailure;
    use crate::workspace::left_dock::git_ops::health::ReadOutcome;
    use gpui::AppContext;

    let temp = tempfile::tempdir().unwrap();
    install_fake_repo(cx, temp.path());
    let (window, ws) = crate::workspace::tests::build_workspace_with(
        cx,
        &daruda_config::Config::default(),
        Some(daruda_store::project::Project::from_path(temp.path())),
    );
    ws.update(cx, |ws, cx| ws.reconcile_bootstrapped_lanes(cx));
    cx.run_until_parked();
    let second = cx
        .update_window(window.into(), |_, window, cx| {
            ws.update(cx, |ws, cx| {
                let first = ws.active;
                let project = ws.active_project_mut().unwrap();
                let id = project.lanes.iter().map(|lane| lane.id).max().unwrap() + 1;
                let path = project.lanes[0].path.clone();
                project.lanes.push(crate::lane::Lane::git(
                    id,
                    path.clone(),
                    Some("side".into()),
                    path.clone(),
                    path,
                    1,
                ));
                let second = LaneRef {
                    project: project.id,
                    lane: id,
                };
                let now = cx.background_executor().now();
                let permit = ws.git.health.admit(ReadOrigin::Automatic, now).unwrap();
                ws.git.health.finish(
                    permit,
                    ReadOutcome::Environment(GitEnvironmentFailure::ExecutableMissing),
                    now,
                );
                ws.run_git_axis(
                    ReadRequest {
                        target: first,
                        recovery_origin: Some(ReadOrigin::Manual),
                    },
                    |git| &mut git.worktree_refresh,
                    || Err::<(), _>(GitError::NotFound),
                    |_, _, _| false,
                    no_repeat,
                    cx,
                );
                ws.activate_lane(second, window, cx);
                ws.refresh_git_status_manual(second, cx);
                assert!(
                    ws.lane_scoped[&second].git.manual_refresh_pending,
                    "a different lane's probe must not consume this request"
                );
                second
            })
        })
        .unwrap();
    // No clock advance: the queued manual request must bypass the new cooldown.
    cx.run_until_parked();
    ws.read_with(cx, |ws, _| {
        assert!(!ws.git.health.is_blocked());
        assert!(ws.lane_git_worktree(second).is_some());
        assert!(!ws.lane_scoped[&second].git.manual_refresh_pending);
    });
}

#[gpui::test]
fn git_refresh_health_tracking_failure_reports_the_whole_outage(cx: &mut TestAppContext) {
    use daruda_store::observability::error_report::ErrorSeverity;

    let temp = tempfile::tempdir().unwrap();
    crate::test_support::fake_git(cx);
    let (_window, ws) = crate::workspace::tests::build_workspace_with(
        cx,
        &daruda_config::Config::default(),
        Some(daruda_store::project::Project::from_path(temp.path())),
    );
    cx.run_until_parked();
    let before = ws.read_with(cx, |ws, _| ws.error_history().len());
    ws.update(cx, |ws, cx| {
        ws.run_git_axis(
            ReadRequest {
                target: ws.active,
                recovery_origin: None,
            },
            |git| &mut git.tracking_refresh,
            || Err::<(), _>(GitError::NotFound),
            |ws, result, cx| {
                if let Err(error) = result {
                    ws.report_error(
                        ErrorReport::new("Tracking failed")
                            .severity(ErrorSeverity::Warning)
                            .from_error(&error)
                            .build(),
                        cx,
                    );
                }
                false
            },
            no_repeat,
            cx,
        );
    });
    cx.run_until_parked();
    ws.read_with(cx, |ws, _| {
        assert_eq!(ws.error_history().len(), before + 1);
        let report = &ws.error_history()[0];
        assert_eq!(report.severity, ErrorSeverity::Error);
        assert_eq!(report.dedup_key.as_deref(), Some("git.status"));
        assert!(
            report
                .message
                .contains("git is not installed or not on PATH")
        );
    });
}

#[gpui::test]
fn git_refresh_health_repository_failure_keeps_its_axis_diagnostic(cx: &mut TestAppContext) {
    use daruda_store::observability::error_report::ErrorSeverity;

    let temp = tempfile::tempdir().unwrap();
    crate::test_support::fake_git(cx);
    let (_window, ws) = crate::workspace::tests::build_workspace_with(
        cx,
        &daruda_config::Config::default(),
        Some(daruda_store::project::Project::from_path(temp.path())),
    );
    cx.run_until_parked();
    let before = ws.read_with(cx, |ws, _| ws.error_history().len());
    ws.update(cx, |ws, cx| {
        ws.run_git_axis(
            ReadRequest {
                target: ws.active,
                recovery_origin: None,
            },
            |git| &mut git.tracking_refresh,
            || {
                Err::<(), _>(GitError::Exit {
                    code: Some(128),
                    stderr: "fatal: not a git repository".into(),
                })
            },
            |ws, result, cx| {
                if let Err(error) = result {
                    ws.report_error(
                        ErrorReport::new("Tracking failed")
                            .severity(ErrorSeverity::Warning)
                            .from_error(&error)
                            .dedup("git.tracking")
                            .build(),
                        cx,
                    );
                }
                false
            },
            no_repeat,
            cx,
        );
    });
    cx.run_until_parked();
    ws.read_with(cx, |ws, _| {
        assert!(!ws.git.health.is_blocked());
        assert_eq!(ws.error_history().len(), before + 1);
        let report = &ws.error_history()[0];
        assert_eq!(report.severity, ErrorSeverity::Warning);
        assert_eq!(report.dedup_key.as_deref(), Some("git.tracking"));
    });
}

#[gpui::test]
fn git_refresh_health_blocks_launches_and_reports_until_recovery(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    crate::test_support::fake_git(cx);
    let (_window, ws) = crate::workspace::tests::build_workspace_with(
        cx,
        &daruda_config::Config::default(),
        Some(daruda_store::project::Project::from_path(temp.path())),
    );
    cx.run_until_parked();
    let reads = Arc::new(AtomicUsize::new(0));
    let run = |origin, fail, cx: &mut TestAppContext| {
        let reads = reads.clone();
        ws.update(cx, |ws, cx| {
            ws.run_git_axis(
                ReadRequest {
                    target: ws.active,
                    recovery_origin: Some(origin),
                },
                |git| &mut git.worktree_refresh,
                move || {
                    reads.fetch_add(1, Ordering::SeqCst);
                    if fail {
                        Err(GitError::NotFound)
                    } else {
                        Ok(())
                    }
                },
                |ws, result, cx| {
                    if let Err(e) = result {
                        ws.report_error(
                            ErrorReport::new("Test Git failure").from_error(&e).build(),
                            cx,
                        );
                    }
                    false
                },
                no_repeat,
                cx,
            );
        });
        cx.run_until_parked();
    };
    let before = ws.read_with(cx, |ws, _| ws.error_history().len());
    run(ReadOrigin::Automatic, true, cx);
    run(ReadOrigin::Automatic, true, cx);
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    let toast = ws.read_with(cx, |ws, cx| ws.error_toasts(cx).iter().last().unwrap().id);
    ws.update(cx, |ws, cx| ws.dismiss_error_toast(toast, cx));
    run(ReadOrigin::Manual, true, cx);
    assert!(ws.read_with(cx, |ws, cx| ws.error_toasts(cx).is_empty()));
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    assert_eq!(
        ws.read_with(cx, |ws, _| ws.error_history().len()),
        before + 1
    );
    run(ReadOrigin::Manual, false, cx);
    assert!(!ws.read_with(cx, |ws, _| ws.git.health.is_blocked()));
    run(ReadOrigin::Automatic, true, cx);
    assert_eq!(
        ws.read_with(cx, |ws, _| ws.error_history().len()),
        before + 2
    );
}

#[gpui::test]
fn git_refresh_health_drops_old_results_and_releases_slots(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    crate::test_support::fake_git(cx);
    let (_window, ws) = crate::workspace::tests::build_workspace_with(
        cx,
        &daruda_config::Config::default(),
        Some(daruda_store::project::Project::from_path(temp.path())),
    );
    cx.run_until_parked();
    let applied = Arc::new(AtomicUsize::new(0));
    ws.update(cx, |ws, cx| {
        for tracking in [false, true] {
            let applied = applied.clone();
            ws.run_git_axis(
                ReadRequest {
                    target: ws.active,
                    recovery_origin: None,
                },
                if tracking {
                    |git| &mut git.tracking_refresh
                } else {
                    |git| &mut git.worktree_refresh
                },
                || Err::<(), _>(GitError::NotFound),
                move |_, _, _| {
                    applied.fetch_add(1, Ordering::SeqCst);
                    false
                },
                no_repeat,
                cx,
            );
        }
    });
    cx.run_until_parked();
    assert_eq!(applied.load(Ordering::SeqCst), 0);
    ws.read_with(cx, |ws, _| {
        let state = &ws.lane_scoped[&ws.active].git;
        assert_eq!(
            state.tracking_refresh,
            crate::workspace::lane_scoped::RefreshSlot::Idle
        );
        assert_eq!(
            state.worktree_refresh,
            crate::workspace::lane_scoped::RefreshSlot::Idle
        );
        assert!(ws.git.health.retry_at().is_some());
    });
    cx.executor().advance_clock(COOLDOWN);
}

#[gpui::test]
fn git_refresh_health_timer_recovers_and_reprobes_directories(cx: &mut TestAppContext) {
    use crate::lane::git::GitEnvironmentFailure;
    use crate::workspace::lane_scoped::GitDirsState;
    use crate::workspace::left_dock::git_ops::health::ReadOutcome;
    let temp = tempfile::tempdir().unwrap();
    install_fake_repo(cx, temp.path());
    let (_window, ws) = crate::workspace::tests::build_workspace_with(
        cx,
        &daruda_config::Config::default(),
        Some(daruda_store::project::Project::from_path(temp.path())),
    );
    ws.update(cx, |ws, cx| ws.reconcile_bootstrapped_lanes(cx));
    cx.run_until_parked();
    ws.update(cx, |ws, cx| {
        let now = cx.background_executor().now();
        let permit = ws.git.health.admit(ReadOrigin::Automatic, now).unwrap();
        ws.git.health.finish(
            permit,
            ReadOutcome::Environment(GitEnvironmentFailure::ExecutableMissing),
            now,
        );
        let active = ws.active;
        let git = &mut ws.lane_scoped_mut(active).git;
        git.worktree = None;
        git.dirs = GitDirsState::Unavailable;
        ws.schedule_git_recovery(cx);
    });
    cx.run_until_parked();
    cx.executor().advance_clock(COOLDOWN);
    cx.run_until_parked();
    ws.read_with(cx, |ws, _| {
        assert!(!ws.git.health.is_blocked());
        let git = &ws.lane_scoped[&ws.active].git;
        assert!(git.worktree.is_some());
        assert!(matches!(git.dirs, GitDirsState::Known(_)));
    });
}
