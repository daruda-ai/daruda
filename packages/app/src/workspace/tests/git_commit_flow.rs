//! The Git panel's commit and push buttons: whether they ask first, and what
//! an empty commit message commits as. Driven against a scripted git; what a
//! real commit and push do is `daruda_project`'s `tests/it/git_remote.rs`.

use super::*;
use crate::lane::git::{GitCall, GitFileEntry, GitWorktreeStatus};
use crate::workspace::{CommitChanges, PushChanges};

/// A repository with `staged` files staged, the workspace over it (Root-hosted
/// like the app: the commit confirm is a Root dialog), its status read.
fn workspace_on(
    cx: &mut TestAppContext,
    staged: &[&str],
    config: daruda_config::Config,
) -> GitFixture {
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
    let f = GitFixture::open(cx, &config, |repo| repo.status(status));
    let id = f.ws.read_with(cx, |ws, _| ws.active_ref());
    f.ws.update(cx, |ws, cx| ws.refresh_git_status(id, cx));
    cx.run_until_parked();
    f
}

fn commit(f: &GitFixture, cx: &mut TestAppContext, message: &str) {
    let message = message.to_string();
    cx.update_window(f.window.into(), |_, window, cx| {
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
fn commits(f: &GitFixture) -> Vec<String> {
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
    cx.update_window(f.window.into(), |_, window, cx| {
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
