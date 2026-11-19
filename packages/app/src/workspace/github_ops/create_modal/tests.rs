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
    // As the app does at startup: the form asks GitHub about a base.
    cx.update(crate::gh_host::GhHost::init_if_missing);
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

/// Start over puts back what the form opened with: the commit's subject
/// as the title, not an empty one.
#[gpui::test]
fn starting_over_fills_the_title_from_the_commit_again(cx: &mut TestAppContext) {
    let repo = FakeRepo::at("/fake/repo").head_message("Fix the sidebar");
    let (window, modal) = build(cx, repo, Some("main"));
    window
        .update(cx, |modal, window, cx| {
            modal
                .title
                .update(cx, |input, cx| input.set_value("Mine", window, cx));
        })
        .unwrap();
    cx.run_until_parked();
    let title = |cx: &mut TestAppContext| {
        modal.read_with(cx, |modal, cx| modal.title.read(cx).value().to_string())
    };
    assert_eq!(title(cx), "Mine");
    window
        .update(cx, |modal, window, cx| modal.discard_for_test(window, cx))
        .unwrap();
    cx.run_until_parked();
    assert_eq!(title(cx), "Fix the sidebar");
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

#[gpui::test]
fn the_form_says_what_it_publishes_and_follows_the_base(cx: &mut TestAppContext) {
    use crate::lane::git::PushTarget;
    use summary::{Against, Expected, ShownTarget};
    let push = PushTarget {
        remote: "origin".into(),
        url: "git@github.com:o/r.git".into(),
        branch: "feat".into(),
        published: true,
        has_upstream: true,
        review: None,
    };
    let repo = FakeRepo::at("/fake/repo").push_target(push.clone());
    let (window, modal) = build(cx, repo, Some("mian"));
    let against = modal.read_with(cx, |modal, cx| modal.current_against(cx).cloned());
    assert_eq!(against, Some(Against::BaseNotFound("mian".into())));

    // Typing a base: the old comparison is not shown for it, and Enter
    // right away still checks the target the form showed.
    window
        .update(cx, |modal, window, cx| {
            modal
                .title
                .update(cx, |input, cx| input.set_value("Add it", window, cx));
            modal
                .base
                .update(cx, |input, cx| input.set_value("", window, cx));
        })
        .unwrap();
    assert_eq!(
        modal.read_with(cx, |modal, cx| modal.current_against(cx).cloned()),
        None,
        "not read for this base yet"
    );
    let request = modal.read_with(cx, |modal, cx| modal.request(cx)).unwrap();
    assert_eq!(request.expected, Expected::Target(ShownTarget::of(&push)));

    cx.executor().advance_clock(SUMMARY_AFTER);
    cx.run_until_parked();
    assert_eq!(
        modal.read_with(cx, |modal, cx| modal.current_against(cx).cloned()),
        Some(Against::DefaultBase)
    );
}

#[gpui::test]
fn a_form_that_has_not_shown_its_target_creates_nothing(cx: &mut TestAppContext) {
    crate::test_support::init_gpui_component(cx);
    // As the app does at startup: the form asks GitHub about a base.
    cx.update(crate::gh_host::GhHost::init_if_missing);
    let git = FakeGit::new().with(FakeRepo::at("/fake/repo"));
    let handle = git.open(Path::new("/fake/repo"));
    let window = cx.add_window(|window, cx| {
        let lane = daruda_store::project::LaneRef {
            project: 0,
            lane: 0,
        };
        CreatePrModal::new(WeakEntity::new_invalid(), lane, handle, None, window, cx)
    });
    // Before git answers.
    window
        .update(cx, |modal, window, cx| {
            modal
                .title
                .update(cx, |input, cx| input.set_value("Add it", window, cx));
            modal.submit(window, cx);
            assert_eq!(modal.state, CreateState::Editing, "nothing started");
        })
        .unwrap();
}

/// A worktree at a real directory, with templates there.
fn with_templates(
    cx: &mut TestAppContext,
    files: &[(&str, &str)],
) -> (
    tempfile::TempDir,
    WindowHandle<CreatePrModal>,
    Entity<CreatePrModal>,
) {
    let root = tempfile::tempdir().unwrap();
    for (path, body) in files {
        let path = root.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }
    let repo = FakeRepo::at(root.path())
        .head_message("Late subject")
        .push_target(crate::lane::git::PushTarget {
            remote: "origin".into(),
            url: "git@github.com:o/r.git".into(),
            branch: "feat".into(),
            published: true,
            has_upstream: true,
            review: None,
        })
        .remote_branches("origin", &["develop", "feat", "main", "release/1.0"]);
    crate::test_support::init_gpui_component(cx);
    // As the app does at startup: the form asks GitHub about a base.
    cx.update(crate::gh_host::GhHost::init_if_missing);
    let git = FakeGit::new().with(repo);
    let handle = git.open(root.path());
    let window = cx.add_window(|window, cx| {
        let lane = daruda_store::project::LaneRef {
            project: 0,
            lane: 0,
        };
        CreatePrModal::new(WeakEntity::new_invalid(), lane, handle, None, window, cx)
    });
    let modal = window.root(cx).unwrap();
    (root, window, modal)
}

fn body(modal: &Entity<CreatePrModal>, cx: &mut TestAppContext) -> String {
    modal.read_with(cx, |modal, cx| modal.body.read(cx).value().to_string())
}

#[gpui::test]
fn the_default_template_fills_an_untouched_description(cx: &mut TestAppContext) {
    let (_root, window, modal) = with_templates(
        cx,
        &[
            (".github/pull_request_template.md", "## Why"),
            (".github/PULL_REQUEST_TEMPLATE/bug.md", "## Bug"),
        ],
    );
    cx.run_until_parked();
    assert_eq!(body(&modal, cx), "## Why");
    assert_eq!(
        modal.read_with(cx, |modal, cx| modal.request(cx).map(|r| r.title)),
        Some("Late subject".into()),
        "an untouched title takes the subject"
    );

    // Another template replaces one the user did not edit.
    window
        .update(cx, |modal, window, cx| {
            modal.use_template(".github/PULL_REQUEST_TEMPLATE/bug.md", window, cx)
        })
        .unwrap();
    assert_eq!(body(&modal, cx), "## Bug");

    // Written by the user: no template replaces it.
    window
        .update(cx, |modal, window, cx| {
            modal
                .body
                .update(cx, |input, cx| input.set_value("Mine", window, cx));
        })
        .unwrap();
    window
        .update(cx, |modal, window, cx| {
            modal.use_template(".github/pull_request_template.md", window, cx);
        })
        .unwrap();
    assert_eq!(body(&modal, cx), "Mine");
}

/// Typed and cleared before git and the templates answered: the empty
/// title and description stay empty.
#[gpui::test]
fn clearing_a_field_survives_a_late_seed(cx: &mut TestAppContext) {
    let (_root, window, modal) = with_templates(
        cx,
        &[
            ("pull_request_template.md", "## Why"),
            (".github/PULL_REQUEST_TEMPLATE/x.md", "## X"),
        ],
    );
    window
        .update(cx, |modal, window, cx| {
            modal
                .title
                .update(cx, |input, cx| input.set_value("x", window, cx));
            modal
                .title
                .update(cx, |input, cx| input.set_value("", window, cx));
            modal
                .body
                .update(cx, |input, cx| input.set_value("y", window, cx));
            modal
                .body
                .update(cx, |input, cx| input.set_value("", window, cx));
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        modal.read_with(cx, |modal, cx| modal.title.read(cx).value().to_string()),
        ""
    );
    assert_eq!(body(&modal, cx), "");
    // Cleared, a template can still be chosen deliberately.
    window
        .update(cx, |modal, window, cx| {
            modal.use_template("pull_request_template.md", window, cx)
        })
        .unwrap();
    assert_eq!(body(&modal, cx), "## Why");
    // A template is not the user's text: another can replace it.
    window
        .update(cx, |modal, window, cx| {
            modal.use_template(".github/PULL_REQUEST_TEMPLATE/x.md", window, cx)
        })
        .unwrap();
    assert_eq!(body(&modal, cx), "## X");
}

#[gpui::test]
fn the_base_is_searched_among_the_remotes_branches(cx: &mut TestAppContext) {
    let (_root, window, modal) = with_templates(cx, &[]);
    cx.run_until_parked();
    let suggested = |modal: &Entity<CreatePrModal>, cx: &mut TestAppContext| {
        modal.read_with(cx, |modal, cx| modal.base_suggestions(cx))
    };
    assert_eq!(
        suggested(&modal, cx),
        ["develop", "main", "release/1.0"],
        "the head is not its own base"
    );
    // Names starting with what is typed come first.
    window
        .update(cx, |modal, window, cx| {
            modal
                .base
                .update(cx, |input, cx| input.set_value("e", window, cx));
        })
        .unwrap();
    assert_eq!(suggested(&modal, cx), ["develop", "release/1.0"]);
    window
        .update(cx, |modal, window, cx| {
            modal
                .base
                .update(cx, |input, cx| input.set_value("REL", window, cx));
        })
        .unwrap();
    assert_eq!(suggested(&modal, cx), ["release/1.0"]);
    window
        .update(cx, |modal, window, cx| {
            modal.choose_base("release/1.0".into(), window, cx)
        })
        .unwrap();
    assert_eq!(
        modal.read_with(cx, |modal, cx| modal.request(cx).and_then(|r| r.base)),
        Some("release/1.0".into())
    );
    assert!(
        suggested(&modal, cx).is_empty(),
        "the one chosen is not offered again"
    );
}

/// No account known on the host: the form says what it types is not kept.
#[gpui::test]
fn without_an_account_the_form_says_nothing_is_kept(cx: &mut TestAppContext) {
    let (_root, _window, modal) = with_templates(cx, &[]);
    cx.run_until_parked();
    assert_eq!(
        modal.read_with(cx, |modal, cx| modal.creation_keeping(cx)),
        crate::github::DraftKeeping::NoAccount
    );
}
