use daruda_acp::{QuestionDelivery, QuestionOption};

use super::*;

pub(in crate::view::questions) fn option(value: &str) -> QuestionOption {
    QuestionOption {
        value: value.into(),
        label: value.into(),
        description: None,
    }
}

pub(in crate::view::questions) fn field(
    key: &str,
    kind: QuestionFieldKind,
    companion_of: Option<&str>,
) -> QuestionField {
    QuestionField {
        key: key.into(),
        title: None,
        description: None,
        required: false,
        kind,
        companion_of: companion_of.map(str::to_owned),
        default: None,
    }
}

pub(in crate::view::questions) fn request(fields: Vec<QuestionField>) -> QuestionRequest {
    QuestionRequest {
        message: "?".into(),
        tool_call_id: None,
        delivery: QuestionDelivery::Blocking,
        fields,
    }
}

fn select_with_other() -> QuestionRequest {
    request(vec![
        field(
            "question_0",
            QuestionFieldKind::SingleSelect(vec![option("Clippy"), option("Test")]),
            None,
        ),
        field(
            "question_0_custom",
            QuestionFieldKind::Text,
            Some("question_0"),
        ),
    ])
}

#[test]
fn single_pick_replaces_and_multi_pick_toggles() {
    let single = field(
        "a",
        QuestionFieldKind::SingleSelect(vec![option("x"), option("y")]),
        None,
    );
    let multi = field(
        "b",
        QuestionFieldKind::MultiSelect(vec![option("x"), option("y")]),
        None,
    );
    let mut draft = QuestionDraft::default();
    draft.pick(&single, "x");
    draft.pick(&single, "y");
    assert!(!draft.is_picked("a", "x") && draft.is_picked("a", "y"));
    draft.pick(&multi, "x");
    draft.pick(&multi, "y");
    draft.pick(&multi, "x");
    assert!(!draft.is_picked("b", "x") && draft.is_picked("b", "y"));
    assert!(draft.touched());
}

#[test]
fn typed_text_alone_answers_a_select_through_its_companion() {
    let mut request = select_with_other();
    request.fields[0].required = true;
    let mut draft = QuestionDraft::default();
    assert_eq!(draft.answer(&request), None);
    draft.commit_text("question_0_custom", "  something else ");
    assert!(draft.is_answered(&request, "question_0"));
    let answer = draft.answer(&request).expect("answered");
    assert_eq!(answer.len(), 1);
    assert_eq!(
        answer["question_0_custom"],
        QuestionValue::Text("something else".into())
    );
}

#[test]
fn a_pick_plus_text_sends_both() {
    let request = select_with_other();
    let mut draft = QuestionDraft::default();
    draft.pick(&request.fields[0], "Clippy");
    draft.commit_text("question_0_custom", "all-feature");
    let answer = draft.answer(&request).expect("answered");
    assert_eq!(answer["question_0"], QuestionValue::Text("Clippy".into()));
    assert_eq!(
        answer["question_0_custom"],
        QuestionValue::Text("all-feature".into())
    );
}

#[test]
fn defaults_seed_but_do_not_count_as_touched() {
    let mut request = request(vec![
        field(
            "s",
            QuestionFieldKind::SingleSelect(vec![option("x")]),
            None,
        ),
        field(
            "m",
            QuestionFieldKind::MultiSelect(vec![option("x"), option("y")]),
            None,
        ),
        field("b", QuestionFieldKind::Boolean, None),
    ]);
    request.fields[0].default = Some(QuestionValue::Text("x".into()));
    request.fields[1].default = Some(QuestionValue::Choices(vec!["y".into(), "gone".into()]));
    request.fields[2].default = Some(QuestionValue::Bool(true));
    let draft = QuestionDraft::seeded(&request);
    assert!(!draft.touched());
    assert!(draft.is_picked("s", "x"));
    assert!(draft.is_picked("m", "y") && !draft.is_picked("m", "gone"));
    assert_eq!(draft.flag("b"), Some(true));
    assert!(draft.answer(&request).is_some());
}

#[test]
fn empty_commit_removes_text() {
    let mut draft = QuestionDraft::default();
    draft.commit_text("k", "hi");
    assert_eq!(draft.text("k"), Some("hi"));
    draft.commit_text("k", "   ");
    assert_eq!(draft.text("k"), None);
}

#[test]
fn unanswered_optional_fields_are_absent_and_required_blocks() {
    let mut request = request(vec![
        field("t", QuestionFieldKind::Text, None),
        field("b", QuestionFieldKind::Boolean, None),
    ]);
    let mut draft = QuestionDraft::default();
    assert_eq!(draft.answer(&request), None);
    draft.set_flag("b", false);
    let answer = draft.answer(&request).expect("answered");
    assert_eq!(answer.keys().collect::<Vec<_>>(), ["b"]);
    request.fields[0].required = true;
    assert_eq!(draft.answer(&request), None);
}

#[test]
fn picks_answer_is_one_select_and_its_companion() {
    assert!(picks_answer(&select_with_other()));
    let mut two = select_with_other();
    two.fields
        .push(field("q1", QuestionFieldKind::SingleSelect(vec![]), None));
    assert!(!picks_answer(&two));
}

#[test]
fn companion_key_and_primary_fields() {
    let request = select_with_other();
    assert_eq!(
        companion_key(&request, "question_0"),
        Some("question_0_custom")
    );
    assert_eq!(companion_key(&request, "question_0_custom"), None);
    assert_eq!(primary_fields(&request).count(), 1);
}

/// Each row: the field's title, the request's message, and the header.
/// Whitespace is no header: an agent that sends neither still gets a
/// stand-in, so nothing that names the question names it with nothing.
#[test]
fn a_header_falls_back_to_the_message_then_a_stand_in() {
    let untitled = crate::strings::agent_chat::question_untitled();
    let rows: Vec<(Option<&str>, &str, &str)> = vec![
        (Some("Which branch?"), "Pick one", "Which branch?"),
        (None, "Pick one", "Pick one"),
        (Some(""), "Pick one", "Pick one"),
        (Some("  "), "Pick one", "Pick one"),
        (None, "", &untitled),
        (Some(" "), "\n", &untitled),
    ];
    for (title, message, want) in rows {
        let mut f = field("pick", QuestionFieldKind::Text, None);
        f.title = title.map(str::to_owned);
        let mut q = request(vec![f.clone()]);
        q.message = message.into();
        assert_eq!(field_header(&q, Some(&f)), want, "{title:?} / {message:?}");
    }
    let mut q = request(Vec::new());
    q.message = String::new();
    assert_eq!(field_header(&q, None), untitled, "no field at all");
}
