//! The phone answering the field the composer holds unsent text for: that
//! text replaces the phone's answer only when sent on purpose, twice.

use std::collections::BTreeMap;

use daruda_acp::{QuestionAnswer, QuestionFieldKind, QuestionRequest, QuestionValue};
use daruda_chat::view::{AgentChatView, PhoneAnswered, PhoneReply};
use gpui::{Entity, TestAppContext, VisualTestContext};

use super::composer::{
    add_pane, ask, composer_text, connect, field, press_enter_key, strip, type_text, workspace,
};
use crate::workspace::Workspace;
use crate::workspace::main_area::pane_tree::PaneId;

/// Two free-text questions.
fn path_then_reason() -> QuestionRequest {
    QuestionRequest {
        message: "Where, and why?".into(),
        tool_call_id: None,
        delivery: daruda_acp::QuestionDelivery::Blocking,
        fields: vec![
            field("path", "Path", QuestionFieldKind::Text, None),
            field("why", "Why", QuestionFieldKind::Text, None),
        ],
    }
}

fn view(
    ws: &Entity<Workspace>,
    vcx: &mut VisualTestContext,
    pane: PaneId,
) -> Entity<AgentChatView> {
    vcx.update(|_, cx| ws.read(cx).agent_chat_view(pane).expect("chat").clone())
}

/// The phone answers the path, as the person types one on the desktop.
fn phone_answers_path(view: &Entity<AgentChatView>, vcx: &mut VisualTestContext) {
    vcx.update(|_, cx| {
        view.update(cx, |v, cx| {
            v.take_questions_untold_to_phone();
            v.answer_question_from_phone(1, 0, PhoneReply::Text("phone/".into()), cx);
        })
    });
    vcx.run_until_parked();
}

fn path(view: &Entity<AgentChatView>, vcx: &mut VisualTestContext) -> Option<String> {
    vcx.update(|_, cx| view.read(cx).question_field_text(1, 0))
}

fn phone_answered(ws: &Entity<Workspace>, vcx: &mut VisualTestContext) -> Option<PhoneAnswered> {
    strip(ws, vcx).expect("strip").composer.phone_answered
}

/// The first Enter keeps the text and says the phone answered; the second
/// replaces that answer and moves on.
#[gpui::test]
fn a_second_enter_replaces_a_phone_answer_given_while_typing(cx: &mut TestAppContext) {
    let (_dir, ws, mut vcx) = workspace(cx);
    let pane = add_pane(&ws, &mut vcx);
    let mut probes = connect(&ws, &mut vcx, pane, &[1]);
    ask(&ws, &mut vcx, pane, 1, path_then_reason());
    let view = view(&ws, &mut vcx, pane);
    type_text(&ws, &mut vcx, "desk/");
    phone_answers_path(&view, &mut vcx);
    assert_eq!(
        phone_answered(&ws, &mut vcx),
        Some(PhoneAnswered {
            field: 0,
            replace_armed: false
        })
    );

    press_enter_key(&mut vcx);
    assert_eq!(composer_text(&ws, &mut vcx), "desk/", "the text stays");
    assert_eq!(path(&view, &mut vcx).as_deref(), Some("phone/"));
    assert!(phone_answered(&ws, &mut vcx).is_some_and(|p| p.replace_armed));

    press_enter_key(&mut vcx);
    assert_eq!(path(&view, &mut vcx).as_deref(), Some("desk/"));
    assert_eq!(composer_text(&ws, &mut vcx), "");
    assert_eq!(phone_answered(&ws, &mut vcx), None);
    assert_eq!(probes[0].answer(), None, "the reason is still unanswered");
}

/// Moving on from the field leaves the phone's answer in place and the
/// text in the composer, which no Enter puts on the next field; emptied,
/// the composer answers that field as usual.
#[gpui::test]
fn a_focus_move_does_not_carry_typed_text_over_a_phone_answer(cx: &mut TestAppContext) {
    let (_dir, ws, mut vcx) = workspace(cx);
    let pane = add_pane(&ws, &mut vcx);
    let mut probes = connect(&ws, &mut vcx, pane, &[1]);
    ask(&ws, &mut vcx, pane, 1, path_then_reason());
    let view = view(&ws, &mut vcx, pane);
    type_text(&ws, &mut vcx, "desk/");
    phone_answers_path(&view, &mut vcx);

    vcx.update(|_, cx| view.update(cx, |v, cx| v.bind_question_composer(1, Some(1), false, cx)));
    vcx.run_until_parked();
    assert_eq!(path(&view, &mut vcx).as_deref(), Some("phone/"));
    assert_eq!(composer_text(&ws, &mut vcx), "desk/");
    let shown = strip(&ws, &mut vcx).expect("strip");
    assert_eq!(shown.composer.field, 1);
    assert_eq!(shown.composer.phone_answered.map(|p| p.field), Some(0));

    press_enter_key(&mut vcx);
    assert_eq!(
        vcx.update(|_, cx| view.read(cx).question_field_text(1, 1)),
        None
    );
    assert_eq!(composer_text(&ws, &mut vcx), "desk/");

    type_text(&ws, &mut vcx, "");
    assert_eq!(phone_answered(&ws, &mut vcx), None);
    type_text(&ws, &mut vcx, "because");
    press_enter_key(&mut vcx);
    assert_eq!(
        probes[0].answer(),
        Some(QuestionAnswer::Submit(BTreeMap::from([
            ("path".to_owned(), QuestionValue::Text("phone/".into())),
            ("why".to_owned(), QuestionValue::Text("because".into())),
        ])))
    );
}
