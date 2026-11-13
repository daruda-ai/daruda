use super::*;
use gpui::TestAppContext;
use std::path::PathBuf;

fn build_modal(cx: &mut TestAppContext) -> gpui::Entity<RemoveWorktreeModal> {
    build_modal_with_branch(cx, Some("feat/sidebar".to_string()))
}

fn build_modal_with_branch(
    cx: &mut TestAppContext,
    branch: Option<String>,
) -> gpui::Entity<RemoveWorktreeModal> {
    crate::test_support::init_gpui_component(cx);
    cx.update(crate::git_host::GitHost::init_if_missing);
    let plan = RemoveWorktreePlan {
        path: PathBuf::from("/tmp/repo-feat"),
        repo_root: PathBuf::from("/tmp/repo"),
        lock_dir: None,
    };
    let wh = cx.add_window(|window, cx| {
        RemoveWorktreeModal::new(
            WeakEntity::new_invalid(),
            LaneRef {
                project: 3,
                lane: 7,
            },
            "feat/sidebar",
            "/tmp/repo-feat",
            plan,
            window,
            cx,
        )
        .with_branch(branch)
    });
    wh.root(cx).unwrap()
}

#[gpui::test]
fn folder_deletion_is_opt_in_and_turning_it_off_clears_branch_deletion(cx: &mut TestAppContext) {
    let modal = build_modal(cx);
    modal.update(cx, |modal, cx| {
        modal.stage = Stage::Ready(LaneRemovalState::Unregistered);
        assert!(!modal.delete_folder);
        modal.toggle_delete_folder(cx);
        modal.toggle_delete_branch(cx);
        assert!(modal.delete_folder && modal.delete_branch_too);
        modal.toggle_delete_folder(cx);
        assert!(!modal.delete_folder && !modal.delete_branch_too);
    });
}

#[gpui::test]
fn branch_failure_removes_the_lane_even_if_the_modal_was_dropped(cx: &mut TestAppContext) {
    use crate::lane::git::{FakeFailure, FakeGit, FakeRepo, GitOp};
    use gpui::AppContext as _;
    use std::sync::Arc;

    for drop_modal in [false, true] {
        let root = PathBuf::from("/tmp/remove-recovery-main");
        let path = PathBuf::from("/tmp/remove-recovery-feature");
        let fake = FakeGit::new().with(FakeRepo::at(&root).local_branches(["feature"]));
        fake.fail(
            &root,
            GitOp::DeleteBranch,
            FakeFailure::Exit {
                code: Some(1),
                stderr: "checked out elsewhere".into(),
            },
        );
        cx.update(|cx| crate::git_host::GitHost::set_global(Arc::new(fake), cx));
        let (window, workspace) = crate::workspace::tests::build_workspace_with(
            cx,
            &daruda_config::Config::default(),
            Some(daruda_store::project::Project::from_path(&root)),
        );
        let target = workspace.update(cx, |ws, _| {
            let project = ws.active_ref().project;
            ws.projects
                .get_mut(project)
                .unwrap()
                .lanes
                .push(crate::lane::Lane::git(
                    99,
                    path.clone(),
                    Some("feature".into()),
                    root.clone(),
                    path.clone(),
                    1,
                ));
            LaneRef { project, lane: 99 }
        });
        let modal = cx
            .update_window(window.into(), |_, window, cx| {
                cx.new(|cx| {
                    RemoveWorktreeModal::new(
                        workspace.downgrade(),
                        target,
                        "feature",
                        path.to_string_lossy().into_owned(),
                        RemoveWorktreePlan {
                            path: path.clone(),
                            repo_root: root.clone(),
                            lock_dir: None,
                        },
                        window,
                        cx,
                    )
                    .with_branch(Some("feature".into()))
                })
            })
            .unwrap();
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| {
            modal.update(cx, |modal, cx| {
                modal.delete_branch_too = true;
                modal.submit(window, cx);
            });
        })
        .unwrap();
        let modal = if drop_modal {
            drop(modal);
            None
        } else {
            Some(modal)
        };
        cx.run_until_parked();
        workspace.read_with(cx, |ws, _| {
            assert!(
                ws.lane_for(target).is_none(),
                "removed worktree must leave the app list"
            )
        });
        if let Some(modal) = modal {
            modal.read_with(cx, |modal, _| {
                assert!(modal.stage == Stage::Branch);
                assert!(modal.error.is_some());
                assert!(!modal.submitting);
            });
        }
    }
}

#[gpui::test]
fn modal_defaults_and_delete_branch_toggle_policy(cx: &mut TestAppContext) {
    let modal = build_modal(cx);
    modal.read_with(cx, |m, _| {
        assert_eq!(
            m.target(),
            LaneRef {
                project: 3,
                lane: 7
            }
        );
        assert!(!m.allow_force());
        assert!(m.error.is_none());
        assert!(!m.submitting);
        // Branch deletion is destructive — must require explicit
        // user opt-in even when a branch is present.
        assert!(!m.delete_branch_too());
    });

    modal.update(cx, |m, cx| m.toggle_delete_branch(cx));
    modal.read_with(cx, |m, _| assert!(m.delete_branch_too()));
    modal.update(cx, |m, cx| m.toggle_delete_branch(cx));
    modal.read_with(cx, |m, _| assert!(!m.delete_branch_too()));

    // Default (non-git) or detached-HEAD lane → no branch
    // to delete → toggle must stay off so the modal doesn't
    // pretend it can clean something up.
    let modal = build_modal_with_branch(cx, None);
    modal.update(cx, |m, cx| m.toggle_delete_branch(cx));
    modal.read_with(cx, |m, _| assert!(!m.delete_branch_too()));
}
