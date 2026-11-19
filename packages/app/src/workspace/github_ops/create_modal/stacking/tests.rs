use std::path::Path;

use daruda_github::fake::{FakeGh, GhOp};
use daruda_github::{CheckRollup, PrState, PrSummary, RepoId};
use gpui::{Entity, TestAppContext, WeakEntity, WindowHandle};

use super::*;
use crate::lane::git::{FakeGit, FakeRepo, Git as _, PushTarget};

type Form = (WindowHandle<CreatePrModal>, Entity<CreatePrModal>);

fn open_pr(number: u64, head: &str) -> PrSummary {
    PrSummary {
        number,
        url: format!("https://github.com/o/r/pull/{number}"),
        title: String::new(),
        state: PrState::Open,
        head_branch: head.into(),
        cross_repo: false,
        checks: CheckRollup::None,
        failed: Vec::new(),
        review: None,
        updated_at: "2026-01-01".into(),
    }
}

/// A form for `feat`, pushing to `url`, whose remote has `main` and
/// `parent`; GitHub has pull request 5 open on `parent`.
fn open(cx: &mut TestAppContext, url: &str, host: &str, base: &str) -> (Form, FakeGh) {
    crate::test_support::init_gpui_component(cx);
    let gh = crate::test_support::fake_gh(cx);
    gh.set_prs(&RepoId::new(host, "o", "r"), vec![open_pr(5, "parent")]);
    let repo = FakeRepo::at("/fake/repo")
        .push_target(PushTarget {
            remote: "origin".into(),
            url: url.into(),
            branch: "feat".into(),
            published: true,
            has_upstream: true,
            review: None,
        })
        .remote_branches("origin", &["feat", "main", "parent"]);
    let handle = FakeGit::new().with(repo).open(Path::new("/fake/repo"));
    let base = Some(base.to_owned());
    let window = cx.add_window(|window, cx| {
        let lane = daruda_store::project::LaneRef {
            project: 0,
            lane: 0,
        };
        CreatePrModal::new(WeakEntity::new_invalid(), lane, handle, base, window, cx)
    });
    let modal = window.root(cx).unwrap();
    cx.run_until_parked();
    ((window, modal), gh)
}

fn offered(form: &Form, cx: &mut TestAppContext) -> Option<u64> {
    form.1
        .read_with(cx, |modal, cx| modal.current_stack_parent(cx))
}

fn asked(form: &Form, cx: &mut TestAppContext) -> Option<u64> {
    form.0
        .update(cx, |modal, window, cx| {
            modal.type_for_test("Title", "", window, cx);
            modal.request(cx).and_then(|request| request.stack_on)
        })
        .unwrap()
}

/// A base that another open pull request heads offers stacking on it;
/// nothing is stacked unless the user asks, and the offer is for that
/// base only.
#[gpui::test]
fn a_base_another_pull_request_heads_offers_stacking_on_it(cx: &mut TestAppContext) {
    let (form, _gh) = open(cx, "git@github.com:o/r.git", "github.com", "parent");
    assert_eq!(offered(&form, cx), Some(5));
    assert_eq!(asked(&form, cx), None, "offered, not asked for");
    form.0
        .update(cx, |modal, _, cx| modal.toggle_stack_on(cx))
        .unwrap();
    assert_eq!(asked(&form, cx), Some(5));

    // Another base: the choice does not carry to it.
    form.0
        .update(cx, |modal, window, cx| {
            modal.choose_base("main".into(), window, cx)
        })
        .unwrap();
    assert_eq!(asked(&form, cx), None, "before the new base is read");
    cx.executor().advance_clock(super::super::SUMMARY_AFTER);
    cx.run_until_parked();
    assert_eq!(offered(&form, cx), None, "no pull request heads main");
    assert_eq!(asked(&form, cx), None);

    // Back to the first base: offered again, but asked for only by a new
    // press, not by the one made before the base changed.
    form.0
        .update(cx, |modal, window, cx| {
            modal.choose_base("parent".into(), window, cx)
        })
        .unwrap();
    cx.executor().advance_clock(super::super::SUMMARY_AFTER);
    cx.run_until_parked();
    assert_eq!(offered(&form, cx), Some(5));
    assert_eq!(asked(&form, cx), None, "the old choice is not revived");
}

/// GitHub is asked only where a stack could be made: a branch of the
/// remote, on github.com.
#[gpui::test]
fn stacking_is_asked_about_only_where_it_could_be_done(cx: &mut TestAppContext) {
    let cases = [
        (
            "another host: its stack endpoints are not known",
            "git@ghe.example.com:o/r.git",
            "ghe.example.com",
            "parent",
        ),
        (
            "a base that is not a branch of the remote",
            "git@github.com:o/r.git",
            "github.com",
            "paren",
        ),
        (
            "the head itself",
            "git@github.com:o/r.git",
            "github.com",
            "feat",
        ),
    ];
    for (label, url, host, base) in cases {
        let (form, gh) = open(cx, url, host, base);
        assert_eq!(offered(&form, cx), None, "{label}");
        assert_eq!(gh.count(GhOp::PrsForBranches), 0, "{label}");
    }
}
