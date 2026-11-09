//! The Git panel's commit and push buttons: whether they ask first, and what
//! an empty commit message commits as. Driven against a scripted git; what a
//! real commit and push do is `daruda_project`'s `tests/it/git_remote.rs`.

use super::*;
use crate::lane::git::{FakeGit, FakeRepo, GitCall, GitFileEntry, GitWorktreeStatus};
use crate::workspace::{CommitChanges, PushChanges};

/// A repository at a canonical temp root with `staged` files staged, the
/// workspace over it, and the fake that records what it was asked to do.
struct Fixture {
    wh: gpui::WindowHandle<gpui_component::Root>,
    ws: gpui::Entity<Workspace>,
    fake: FakeGit,
    root: std::path::PathBuf,
    _temp: tempfile::TempDir,
}

fn workspace_on(
    cx: &mut TestAppContext,
    staged: &[&str],
    config: daruda_config::Config,
) -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    // Lanes carry canonical paths; the fake resolves handles by prefix.
    let root = daruda_core::path::canonicalize(temp.path()).unwrap();
    let status = GitWorktreeStatus {
        staged: staged
            .iter()
            .map(|name| GitFileEntry {
                x: 'M',
                y: ' ',
                path: (*name).into(),
                original_path: None,
            })
            .collect(),
        ..GitWorktreeStatus::default()
    };
    let fake = crate::test_support::fake_git(cx);
    fake.add_repo(FakeRepo::at(&root).branch("main").status(status));
    // Root-hosted like the app: the commit confirm is a Root dialog.
    let project = daruda_store::project::Project::from_path(&root);
    let (wh, ws) = build_workspace_with(cx, &config, Some(project));
    ws.update(cx, |ws, cx| ws.reconcile_bootstrapped_lanes(cx));
    cx.run_until_parked();
    let id = ws.read_with(cx, |ws, _| ws.active_ref());
    ws.update(cx, |ws, cx| ws.refresh_git_status(id, cx));
    cx.run_until_parked();
    Fixture {
        wh,
        ws,
        fake,
        root,
        _temp: temp,
    }
}

fn commit(f: &Fixture, cx: &mut TestAppContext, message: &str) {
    let message = message.to_string();
    cx.update_window(f.wh.into(), |_, window, cx| {
        f.ws.update(cx, |ws, cx| {
            ws.git
                .commit_input
                .update(cx, |panel, cx| panel.set_text(message.as_str(), window, cx));
            ws.on_commit_changes(&CommitChanges, window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
}

/// The messages of every commit the workspace asked git for.
fn commits(f: &Fixture) -> Vec<String> {
    f.fake
        .calls()
        .into_iter()
        .filter_map(|call| match call {
            GitCall::Commit { repo, message } => {
                assert_eq!(repo, f.root, "commits run at the repository root");
                Some(message)
            }
            _ => None,
        })
        .collect()
}

fn config_with(edit: impl FnOnce(&mut daruda_config::GitConfig)) -> daruda_config::Config {
    let mut config = daruda_config::Config::default();
    edit(&mut config.git);
    config
}

#[gpui::test]
fn a_commit_asks_first_by_default(cx: &mut TestAppContext) {
    let f = workspace_on(cx, &["a.txt"], daruda_config::Config::default());
    commit(&f, cx, "mine");
    assert_eq!(
        commits(&f),
        Vec::<String>::new(),
        "nothing lands before the confirm"
    );
}

#[gpui::test]
fn a_commit_with_the_confirm_off_lands_at_once(cx: &mut TestAppContext) {
    let f = workspace_on(cx, &["a.txt"], config_with(|g| g.confirm_commit = false));
    commit(&f, cx, "mine");
    assert_eq!(commits(&f), vec!["mine".to_owned()]);
}

#[gpui::test]
fn an_empty_message_commits_as_an_update_of_the_one_file(cx: &mut TestAppContext) {
    let f = workspace_on(cx, &["a.txt"], config_with(|g| g.confirm_commit = false));
    commit(&f, cx, "  ");
    assert_eq!(commits(&f), vec!["Update a.txt".to_owned()]);
}

#[gpui::test]
fn an_empty_message_names_the_count_of_several_files(cx: &mut TestAppContext) {
    let f = workspace_on(
        cx,
        &["a.txt", "b.txt"],
        config_with(|g| g.confirm_commit = false),
    );
    commit(&f, cx, "");
    assert_eq!(commits(&f), vec!["Update 2 files".to_owned()]);
}

#[gpui::test]
fn an_empty_message_is_refused_with_the_default_off(cx: &mut TestAppContext) {
    let config = config_with(|g| {
        g.confirm_commit = false;
        g.default_commit_message = false;
    });
    let f = workspace_on(cx, &["a.txt"], config);
    commit(&f, cx, "");
    assert_eq!(commits(&f), Vec::<String>::new());
}

#[gpui::test]
fn a_push_with_the_confirm_off_lands_at_once(cx: &mut TestAppContext) {
    let f = workspace_on(cx, &["a.txt"], config_with(|g| g.confirm_push = false));
    cx.update_window(f.wh.into(), |_, window, cx| {
        f.ws.update(cx, |ws, cx| ws.on_push_changes(&PushChanges, window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    let pushes: Vec<_> = f
        .fake
        .calls()
        .into_iter()
        .filter(|call| matches!(call, GitCall::Push { .. }))
        .collect();
    assert_eq!(
        pushes,
        vec![GitCall::Push {
            repo: f.root.clone()
        }],
        "the push ran without a confirm"
    );
}
