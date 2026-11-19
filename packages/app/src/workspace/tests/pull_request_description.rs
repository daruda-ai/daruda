//! The Description tab: the conversation under the description, and
//! editing the description for a viewer GitHub lets edit it.

use daruda_github::fake::{GhCall, GhFailure, GhOp};
use daruda_github::{Conversation, ReviewComment};
use gpui::{Modifiers, TestAppContext, VisualTestContext, px, size};

use super::pull_request_detail::{mergeable, opened, repo};
use crate::workspace::pages::pull_requests::detail::PrTab;
use crate::workspace::pages::pull_requests::detail::description::SaveState;

fn shown(f: &super::GitFixture, vcx: &mut VisualTestContext) -> Option<String> {
    f.ws.read_with(vcx, |ws, _| {
        let detail = ws.pages.pull_requests.detail()?;
        detail.idle_read_body()
    })
}

#[gpui::test]
fn the_conversation_is_read_once_the_tab_is_shown(cx: &mut TestAppContext) {
    let (f, gh, id) = opened(cx, mergeable(7, "feat"));
    gh.set_conversation(
        &repo(),
        7,
        Conversation {
            comments: vec![ReviewComment {
                id: "IC_1".into(),
                author: Some("hanji".into()),
                body: "Thanks!".into(),
                ..Default::default()
            }],
            truncated: true,
        },
    );
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.simulate_resize(size(px(1400.), px(900.)));
    let read = |gh: &daruda_github::fake::FakeGh| {
        gh.calls()
            .iter()
            .filter(|call| matches!(call, GhCall::Conversation { .. }))
            .count()
    };
    assert_eq!(read(&gh), 0, "not before the tab is shown");
    f.ws.update(&mut vcx, |ws, cx| {
        ws.select_pr_detail_tab(id, PrTab::Description, cx)
    });
    vcx.run_until_parked();
    assert_eq!(read(&gh), 1);
    assert!(vcx.debug_bounds("pr-conversation").is_some());
    f.ws.update(&mut vcx, |ws, cx| {
        ws.select_pr_detail_tab(id, PrTab::Checks, cx)
    });
    f.ws.update(&mut vcx, |ws, cx| {
        ws.select_pr_detail_tab(id, PrTab::Description, cx)
    });
    vcx.run_until_parked();
    assert_eq!(read(&gh), 1, "read once");
    assert!(
        vcx.debug_bounds("pr-body-edit").is_none(),
        "GitHub did not let the viewer edit"
    );
}

#[gpui::test]
fn an_edited_description_saves_and_a_failure_keeps_the_text(cx: &mut TestAppContext) {
    let mut detail = mergeable(7, "feat");
    detail.viewer_can = detail
        .viewer_can
        .with(daruda_github::Permission::Update, true);
    detail.body = "Old text".into();
    let (f, gh, id) = opened(cx, detail);
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.simulate_resize(size(px(1400.), px(900.)));
    f.ws.update(&mut vcx, |ws, cx| {
        ws.select_pr_detail_tab(id, PrTab::Description, cx)
    });
    vcx.run_until_parked();
    let edit = vcx.debug_bounds("pr-body-edit").unwrap().center();
    vcx.simulate_click(edit, Modifiers::default());
    vcx.run_until_parked();
    let typed = |f: &super::GitFixture, vcx: &mut VisualTestContext, text: Option<&str>| {
        vcx.update(|window, cx| {
            f.ws.update(cx, |ws, cx| {
                let detail = ws.pages.pull_requests.detail().unwrap();
                let edit = detail.body_edit()?;
                let input = edit.input.clone();
                if let Some(text) = text {
                    input.update(cx, |input, cx| input.set_value(text.to_owned(), window, cx));
                }
                Some((input.read(cx).value().to_string(), edit.state.clone()))
            })
        })
    };
    assert_eq!(
        typed(&f, &mut vcx, None),
        Some(("Old text".into(), SaveState::Editing)),
        "starts from what was read"
    );
    typed(&f, &mut vcx, Some("New text"));
    gh.fail(
        GhOp::EditPrBody,
        GhFailure::Exit {
            code: Some(1),
            stderr: "HTTP 403".into(),
        },
    );
    let save = vcx.debug_bounds("pr-body-save").unwrap().center();
    vcx.simulate_click(save, Modifiers::default());
    vcx.run_until_parked();
    let (text, state) = typed(&f, &mut vcx, None).unwrap();
    assert_eq!(text, "New text", "kept to save again");
    assert!(matches!(state, SaveState::Failed(reason) if reason.contains("HTTP 403")));

    gh.clear_failure(GhOp::EditPrBody);
    let save = vcx.debug_bounds("pr-body-save").unwrap().center();
    vcx.simulate_click(save, Modifiers::default());
    vcx.run_until_parked();
    assert_eq!(typed(&f, &mut vcx, None), None, "saved: the editor closes");
    assert!(
        gh.calls()
            .iter()
            .any(|call| matches!(call, GhCall::EditPrBody { body, .. } if body == "New text"))
    );
    assert_eq!(
        shown(&f, &mut vcx).as_deref(),
        Some("New text"),
        "read again from GitHub"
    );
}

#[gpui::test]
fn a_description_changed_on_github_meanwhile_is_not_overwritten_unasked(cx: &mut TestAppContext) {
    let mut detail = mergeable(7, "feat");
    detail.viewer_can = detail
        .viewer_can
        .with(daruda_github::Permission::Update, true);
    detail.body = "Theirs v1".into();
    let (f, gh, id) = opened(cx, detail.clone());
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.simulate_resize(size(px(1400.), px(900.)));
    f.ws.update(&mut vcx, |ws, cx| {
        ws.select_pr_detail_tab(id, PrTab::Description, cx)
    });
    vcx.run_until_parked();
    vcx.update(|window, cx| f.ws.update(cx, |ws, cx| ws.edit_pr_body(id, window, cx)));
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| {
            let edit = ws
                .pages
                .pull_requests
                .detail()
                .unwrap()
                .body_edit()
                .unwrap();
            let input = edit.input.clone();
            input.update(cx, |input, cx| input.set_value("Mine", window, cx));
        })
    });
    // Someone edits it on GitHub; a refresh brings that in.
    gh.set_pr_detail(
        &repo(),
        7,
        daruda_github::PrDetail {
            body: "Theirs v2".into(),
            ..detail
        },
    );
    f.ws.update(&mut vcx, |ws, cx| ws.refresh_pr_detail(id, cx));
    vcx.run_until_parked();
    let edits = |gh: &daruda_github::fake::FakeGh| {
        gh.calls()
            .iter()
            .filter(|call| matches!(call, GhCall::EditPrBody { .. }))
            .count()
    };
    let state = |f: &super::GitFixture, vcx: &mut VisualTestContext| {
        f.ws.read_with(vcx, |ws, _| {
            let detail = ws.pages.pull_requests.detail().unwrap();
            detail.body_edit().map(|edit| edit.state.clone())
        })
    };
    f.ws.update(&mut vcx, |ws, cx| ws.save_pr_body(id, cx));
    vcx.run_until_parked();
    assert_eq!(
        state(&f, &mut vcx),
        Some(SaveState::Conflict),
        "kept, not sent"
    );
    assert_eq!(edits(&gh), 0);
    let overwrite = vcx.debug_bounds("pr-body-overwrite").unwrap().center();
    vcx.simulate_click(overwrite, Modifiers::default());
    vcx.run_until_parked();
    assert_eq!(edits(&gh), 1, "sent once asked");
    assert_eq!(state(&f, &mut vcx), None);
}

/// Edit is offered while a refresh is out, so it also works then: the
/// screen and the action read the same description.
#[gpui::test]
fn the_description_can_be_edited_while_a_refresh_is_out(cx: &mut TestAppContext) {
    let mut detail = mergeable(7, "feat");
    detail.viewer_can = detail
        .viewer_can
        .with(daruda_github::Permission::Update, true);
    detail.body = "Old text".into();
    let (f, _gh, id) = opened(cx, detail);
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| {
            ws.refresh_pr_detail(id, cx);
            ws.edit_pr_body(id, window, cx);
        })
    });
    let editing = f.ws.read_with(cx, |ws, cx| {
        let detail = ws.pages.pull_requests.detail().unwrap();
        (
            detail.pr_waiting().is_some(),
            detail
                .body_edit()
                .map(|edit| edit.input.read(cx).value().to_string()),
        )
    });
    assert_eq!(
        editing,
        (true, Some("Old text".to_owned())),
        "editing began during the refresh, on the body shown"
    );
}

/// Changed on GitHub after the detail last read it, and not read since:
/// the save reads it again, sends nothing, and asks before replacing it.
#[gpui::test]
fn a_change_on_github_not_read_yet_still_stops_the_save(cx: &mut TestAppContext) {
    let mut detail = mergeable(7, "feat");
    detail.viewer_can = detail
        .viewer_can
        .with(daruda_github::Permission::Update, true);
    detail.body = "Old text".into();
    let (f, gh, id) = opened(cx, detail.clone());
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.update(|window, cx| f.ws.update(cx, |ws, cx| ws.edit_pr_body(id, window, cx)));
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| {
            let edit = ws
                .pages
                .pull_requests
                .detail()
                .unwrap()
                .body_edit()
                .unwrap();
            let input = edit.input.clone();
            input.update(cx, |input, cx| {
                input.set_value("Mine".to_owned(), window, cx)
            });
        })
    });
    let mut theirs = detail;
    theirs.body = "Theirs".into();
    gh.set_pr_detail(&repo(), 7, theirs);
    f.ws.update(cx, |ws, cx| ws.save_pr_body(id, cx));
    cx.run_until_parked();
    assert_eq!(gh.count(GhOp::EditPrBody), 0, "theirs is not replaced");
    let state = f.ws.read_with(cx, |ws, _| {
        let detail = ws.pages.pull_requests.detail().unwrap();
        detail.body_edit().map(|edit| edit.state.clone())
    });
    assert_eq!(state, Some(SaveState::Conflict));

    f.ws.update(cx, |ws, cx| ws.overwrite_pr_body(id, cx));
    cx.run_until_parked();
    assert_eq!(gh.count(GhOp::EditPrBody), 1, "replaced when asked");
}
