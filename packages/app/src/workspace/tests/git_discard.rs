//! The Git panel's discard: one file, or everything, back to HEAD — staged
//! changes included, which the per-file action once refused. Driven against a
//! scripted git; what a real discard leaves on disk is `daruda_project`'s
//! `tests/it/git_discard.rs`.

use super::*;
use crate::lane::git::{GitCall, GitFileEntry, GitOp, GitWorktreeStatus};

fn entry(x: char, y: char, path: &str) -> GitFileEntry {
    GitFileEntry {
        x,
        y,
        path: path.into(),
        original_path: None,
    }
}

/// `staged.txt` modified and staged only, `edited.txt` modified in the
/// worktree, `junk.txt` untracked.
fn changed() -> GitWorktreeStatus {
    GitWorktreeStatus {
        staged: vec![entry('M', ' ', "staged.txt")],
        unstaged: vec![entry(' ', 'M', "edited.txt"), entry('?', '?', "junk.txt")],
        ..GitWorktreeStatus::default()
    }
}

struct Fixture {
    git: GitFixture,
    lane: daruda_store::project::LaneRef,
}

fn open(cx: &mut TestAppContext) -> Fixture {
    let git = GitFixture::open(cx, &daruda_config::Config::default(), |repo| {
        repo.status(changed())
    });
    let lane = git.ws.read_with(cx, |ws, _| ws.active_ref());
    git.ws.update(cx, |ws, cx| ws.refresh_git_status(lane, cx));
    cx.run_until_parked();
    Fixture { git, lane }
}

fn discards(f: &Fixture) -> Vec<GitCall> {
    f.git
        .fake
        .calls()
        .into_iter()
        .filter(|call| matches!(call, GitCall::Discard { .. }))
        .collect()
}

/// Discard `pinned` with git scripted to answer `after` from then on, and
/// check the panel re-read the status rather than keep the one it showed.
fn discard_and_settle(
    f: &Fixture,
    cx: &mut TestAppContext,
    pinned: Vec<std::path::PathBuf>,
    after: GitWorktreeStatus,
) {
    f.git.fake.set_status(&f.git.root, after.clone());
    let reads = f.git.fake.reads(GitOp::WorktreeStatus, &f.git.root);
    f.git
        .ws
        .update(cx, |ws, cx| ws.discard_changes(f.lane, pinned, cx));
    cx.run_until_parked();
    assert!(f.git.fake.reads(GitOp::WorktreeStatus, &f.git.root) > reads);
    f.git.ws.read_with(cx, |ws, _| {
        assert_eq!(ws.lane_git_worktree(f.lane), Some(&after));
    });
}

#[gpui::test]
fn discarding_a_staged_only_file_puts_it_back_at_head(cx: &mut TestAppContext) {
    let f = open(cx);
    let mut after = changed();
    after.staged.clear();
    discard_and_settle(&f, cx, vec!["staged.txt".into()], after);
    assert_eq!(
        discards(&f),
        vec![GitCall::Discard {
            repo: f.git.root.clone(),
            pinned: vec!["staged.txt".into()],
        }],
        "only the one file is asked for"
    );
}

#[gpui::test]
fn discarding_everything_leaves_nothing_to_commit(cx: &mut TestAppContext) {
    let f = open(cx);
    let every = f.git.ws.read_with(cx, |ws, _| {
        let status = ws.lane_git_worktree(f.lane).unwrap();
        status
            .staged
            .iter()
            .chain(&status.unstaged)
            .map(|e| e.path.clone())
            .collect::<Vec<_>>()
    });
    discard_and_settle(&f, cx, every, GitWorktreeStatus::default());
    assert_eq!(
        discards(&f),
        vec![GitCall::Discard {
            repo: f.git.root.clone(),
            pinned: vec!["staged.txt".into(), "edited.txt".into(), "junk.txt".into()],
        }]
    );
}

#[gpui::test]
fn discard_all_asks_before_touching_anything(cx: &mut TestAppContext) {
    let f = open(cx);
    cx.update_window(f.git.window.into(), |_, window, cx| {
        f.git
            .ws
            .update(cx, |ws, cx| ws.on_discard_all(f.lane, window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(
        f.git.fake.calls(),
        Vec::new(),
        "nothing is written before the confirm"
    );
}
