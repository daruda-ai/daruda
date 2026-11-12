use std::path::Path;

use gpui::{TestAppContext, WeakEntity, WindowHandle};

use super::*;
use crate::lane::git::{FakeGit, FakeRepo, Git as _};

fn build(
    cx: &mut TestAppContext,
    repo: FakeRepo,
    base: Option<&str>,
) -> (WindowHandle<CreatePrModal>, Entity<CreatePrModal>) {
    crate::test_support::init_gpui_component(cx);
    let git = FakeGit::new().with(repo);
    let handle = git.open(Path::new("/fake/repo"));
    let base = base.map(str::to_owned);
    let window = cx.add_window(|window, cx| {
        let lane = daruda_store::project::LaneRef {
            project: 0,
            lane: 0,
        };
        CreatePrModal::new(WeakEntity::new_invalid(), lane, handle, base, window, cx)
    });
    let modal = window.root(cx).unwrap();
    cx.run_until_parked();
    (window, modal)
}

#[gpui::test]
fn the_last_commit_subject_fills_the_title(cx: &mut TestAppContext) {
    let repo = FakeRepo::at("/fake/repo").head_message("Fix the sidebar\n\nLonger body");
    let (_, modal) = build(cx, repo, Some("main"));
    let request = modal.read_with(cx, |modal, cx| modal.request(cx)).unwrap();
    assert_eq!(request.title, "Fix the sidebar");
    assert_eq!(request.base.as_deref(), Some("main"));
    assert!(!request.draft);
}

#[gpui::test]
fn a_blank_title_has_no_request_and_a_blank_base_is_the_default(cx: &mut TestAppContext) {
    // No commits: nothing to prefill.
    let (window, modal) = build(cx, FakeRepo::at("/fake/repo"), None);
    assert!(modal.read_with(cx, |modal, cx| modal.request(cx)).is_none());

    window
        .update(cx, |modal, window, cx| {
            modal
                .title
                .update(cx, |input, cx| input.set_value("Add it", window, cx));
            modal.toggle_draft(cx);
        })
        .unwrap();
    let request = modal.read_with(cx, |modal, cx| modal.request(cx)).unwrap();
    assert_eq!(request.base, None, "blank base lets GitHub choose");
    assert!(request.draft);
}

#[gpui::test]
fn submitting_without_a_title_shows_why(cx: &mut TestAppContext) {
    let (window, modal) = build(cx, FakeRepo::at("/fake/repo"), None);
    window
        .update(cx, |modal, window, cx| modal.submit(window, cx))
        .unwrap();
    let state = modal.read_with(cx, |modal, _| modal.state.clone());
    assert!(matches!(state, CreateState::Failed(_)), "{state:?}");
}
