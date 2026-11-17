use super::*;

fn owner(account: &str) -> DraftOwner {
    DraftOwner {
        host: "github.com".into(),
        account: account.into(),
        owner: "o".into(),
        name: "n".into(),
        number: 7,
    }
}

#[test]
fn drafts_round_trip_and_a_missing_file_reads_as_none_in_draft() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        load_review_drafts_in(dir.path()),
        Some(ReviewDrafts::default())
    );
    let mut drafts = ReviewDrafts::default();
    let entry = drafts.entry(&owner("me"));
    entry.description = Some(DescriptionDraft {
        base: "old".into(),
        text: "new".into(),
    });
    entry.replies.push(ReplyDraft {
        thread: "PRRT_1".into(),
        text: "Fixed.".into(),
    });
    entry.comments.push(LineCommentDraft {
        id: entry.next_comment_id(),
        path: "src/a.rs".into(),
        side: DraftSide::Right,
        start: 3,
        end: 5,
        revision: "abc1234".into(),
        body: "Simpler:".into(),
        suggestion: Some("let x = 1;".into()),
    });
    assert_eq!(entry.next_comment_id(), 2);
    save_review_drafts_in(dir.path(), &drafts).unwrap();
    assert_eq!(load_review_drafts_in(dir.path()), Some(drafts));
}

#[test]
fn drafts_are_per_account_and_emptied_ones_are_pruned() {
    let mut drafts = ReviewDrafts::default();
    drafts.entry(&owner("me")).replies.push(ReplyDraft {
        thread: "t".into(),
        text: "x".into(),
    });
    assert!(
        drafts.of(&owner("someone-else")).is_none(),
        "another account sees none"
    );
    drafts.entry(&owner("me")).replies.clear();
    drafts.entry(&owner("other"));
    drafts.prune();
    assert!(drafts.prs.is_empty());
}

#[test]
fn a_newer_or_damaged_file_is_not_read_as_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = review_drafts_path_in(dir.path());
    // A version-1 file, from before line comments, reads.
    std::fs::write(&path, r#"{"schema_version": 1, "prs": []}"#).unwrap();
    let old = load_review_drafts_in(dir.path()).unwrap();
    assert_eq!(
        (old.prs.len(), old.schema_version),
        (0, SCHEMA_VERSION),
        "upgraded on read"
    );
    std::fs::write(&path, r#"{"schema_version": 99, "prs": []}"#).unwrap();
    assert_eq!(load_review_drafts_in(dir.path()), None, "newer");
    std::fs::write(&path, "{ not json").unwrap();
    assert_eq!(load_review_drafts_in(dir.path()), None, "damaged");
}

#[test]
fn a_creation_draft_is_kept_per_branch_apart_from_review_drafts() {
    let dir = tempfile::tempdir().unwrap();
    let owner = |head: &str| CreationOwner {
        host: "github.com".into(),
        account: "al".into(),
        owner: "o".into(),
        name: "r".into(),
        head: head.into(),
    };
    let mut drafts = ReviewDrafts::default();
    drafts.set_creation(
        &owner("feat"),
        Some(CreationDraft {
            of: owner("feat"),
            title: Some(String::new()),
            body: Some("Why".into()),
            base: None,
            draft: true,
            template: None,
            saved_at: 1,
        }),
    );
    // Touched nowhere: nothing to keep.
    drafts.set_creation(
        &owner("other"),
        Some(CreationDraft {
            of: owner("other"),
            title: None,
            body: None,
            base: None,
            draft: false,
            template: None,
            saved_at: 1,
        }),
    );
    drafts.prune();
    save_review_drafts_in(dir.path(), &drafts).unwrap();
    let loaded = load_review_drafts_in(dir.path()).unwrap();
    let kept = loaded.creation(&owner("feat")).unwrap();
    assert_eq!(
        kept.title.as_deref(),
        Some(""),
        "a cleared title stays cleared"
    );
    assert!(loaded.creation(&owner("other")).is_none());
    assert!(loaded.prs.is_empty(), "no review draft came of it");
    let mut dropped = loaded;
    dropped.set_creation(&owner("feat"), None);
    assert!(dropped.creations.is_empty());
}
