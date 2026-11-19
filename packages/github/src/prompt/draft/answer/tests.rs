use super::*;

#[test]
fn an_answer_is_read_through_what_surrounds_it() {
    let read = |title: &str, body: &str| {
        Some(DraftText {
            title: title.into(),
            body: body.into(),
        })
    };
    let cases: [(&str, &str, Option<DraftText>); 12] = [
        (
            "the object alone",
            r#"{"title":"Add x","body":"Why\n\n- a"}"#,
            read("Add x", "Why\n\n- a"),
        ),
        (
            "inside a code fence",
            "```json\n{\"title\": \"Add x\", \"body\": \"b\"}\n```\n",
            read("Add x", "b"),
        ),
        (
            "after a banner that has braces of its own",
            "note {not json}\n{\"title\":\"Add x\",\"body\":\"uses {braces}\"}\ntrailing",
            read("Add x", "uses {braces}"),
        ),
        (
            "no body: the title alone",
            r#"{"title":"Add x"}"#,
            read("Add x", ""),
        ),
        (
            // A newline or an escape in a title would start a line, or
            // reach a terminal, nobody wrote.
            "a title made one clean line",
            "{\"title\":\"  Add\\nx\\u001b[31m  \",\"body\":\"b\"}",
            read("Add x", "b"),
        ),
        (
            // The prompt's example echoed first is not the answer.
            "the example echoed before the answer",
            "{\"title\": \"...\", \"body\": \"...\"}\n{\"title\":\"Add x\",\"body\":\"b\"}",
            read("Add x", "b"),
        ),
        (
            // JSON has no raw line break in a string; agents write them.
            "line breaks left raw in the body",
            "{\"title\":\"Add x\",\"body\":\"Why\n\n- a\tb\"}",
            read("Add x", "Why\n\n- a\tb"),
        ),
        (
            // An object quoted in the body is not a second candidate.
            "an answer that does not parse, quoting an object",
            r#"{"title":"Add x","body":"see {\"title\":\"Wrong\"}",}"#,
            None,
        ),
        (
            "an object without a title, then the answer",
            r#"{"note":{"title":"Wrong"}} {"title":"Add x","body":"b"}"#,
            read("Add x", "b"),
        ),
        ("no title", r#"{"body":"b"}"#, None),
        ("a blank title", r#"{"title":"  ","body":"b"}"#, None),
        ("not JSON", "I could not write one.", None),
    ];
    for (label, output, expected) in cases {
        assert_eq!(parse_pr_draft(output), expected, "{label}");
    }
}

#[test]
fn a_title_and_a_body_are_cut_to_what_github_takes() {
    let output = format!(
        r#"{{"title":"{}","body":"{}"}}"#,
        "가".repeat(300),
        "나".repeat(BODY_CHARS + 10)
    );
    let text = parse_pr_draft(&output).unwrap();
    assert_eq!(text.title.chars().count(), TITLE_CHARS);
    assert_eq!(text.body.chars().count(), BODY_CHARS);
}
