//! `conversation_13506.json` is the real answer to `CONVERSATION_QUERY` for
//! `cli/cli` PR 13506 (gh 2.74.2, 2026-11-18).

use super::*;

fn fixture(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read_to_string(path).unwrap()
}

#[test]
fn the_recorded_conversation_reads_as_comments() {
    let read = read_conversation(|after| {
        assert_eq!(after, None);
        Ok(fixture("conversation_13506.json"))
    })
    .unwrap();
    assert_eq!(read.comments.len(), 2);
    assert!(!read.truncated);
    assert!(
        read.comments.iter().all(|c| c.id.starts_with("IC_")),
        "{:?}",
        read.comments
    );
    assert!(read.comments[0].web_url().is_some());
}

#[test]
fn later_pages_are_read_up_to_the_cap() {
    let page = |next: bool| {
        format!(
            r#"{{"data":{{"repository":{{"pullRequest":{{"comments":{{"pageInfo":{{"hasNextPage":{next},"endCursor":"c"}},"nodes":[{{"id":"IC_1","url":null,"author":null,"body":"x"}}]}}}}}}}}}}"#
        )
    };
    let mut asked = 0;
    let two = read_conversation(|after| {
        asked += 1;
        Ok(page(after.is_none()))
    })
    .unwrap();
    assert_eq!((asked, two.comments.len(), two.truncated), (2, 2, false));
    let capped = read_conversation(|_| Ok(page(true))).unwrap();
    assert_eq!(capped.comments.len(), CONVERSATION_PAGES_MAX);
    assert!(capped.truncated);
    assert!(parse_conversation_page(r#"{"data":{"repository":null}}"#).is_err());
}
