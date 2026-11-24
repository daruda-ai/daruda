use gpui::{TestAppContext, VisualTestContext};

use super::super::super::direct::PrDestination;
use super::*;
use crate::workspace::tests::pull_request_detail::{repo, window};

#[test]
fn a_url_fragment_matches_its_comment_kind_and_entire_number() {
    let mut comment = ReviewComment::default();
    for (fragment, review, conversation) in [
        ("discussion_r12", true, false),
        ("issuecomment-12", false, true),
        ("discussion_r123", false, false),
        ("issuecomment-123", false, false),
    ] {
        comment.url = Some(format!("https://github.com/o/r/pull/7#{fragment}"));
        assert_eq!(
            CommentKind::Review(12).matches(&comment),
            review,
            "{fragment}"
        );
        assert_eq!(
            CommentKind::Conversation(12).matches(&comment),
            conversation,
            "{fragment}"
        );
    }
}

#[gpui::test]
fn a_comment_link_reveals_the_exact_reply_or_offers_the_original_url(cx: &mut TestAppContext) {
    use daruda_github::{
        Conversation, DiffSide, FileChange, PrFile, PrFiles, ReviewComment, ReviewThread,
        ReviewThreads,
    };

    let (f, gh, _) = window(cx);
    let comment = |node: &str, fragment: &str| ReviewComment {
        id: node.into(),
        body: node.into(),
        url: Some(format!("https://github.com/o/r/pull/7#{fragment}")),
        ..Default::default()
    };
    gh.set_review_threads(
        &repo(),
        7,
        ReviewThreads {
            threads: vec![
                ReviewThread {
                    id: "current".into(),
                    path: "src.rs".into(),
                    lines: Some((1, 1)),
                    side: DiffSide::Right,
                    comments: vec![
                        comment("first", "discussion_r100"),
                        comment("reply", "discussion_r200"),
                    ],
                    ..Default::default()
                },
                ReviewThread {
                    id: "outdated".into(),
                    outdated: true,
                    comments: vec![comment("old", "discussion_r300")],
                    ..Default::default()
                },
            ],
            truncated: false,
        },
    );
    gh.set_pr_files(
        &repo(),
        7,
        PrFiles {
            files: vec![PrFile {
                path: "src.rs".into(),
                change: FileChange::Added,
                additions: 1,
                deletions: 0,
                patch: Some("@@ -0,0 +1 @@\n+new".into()),
                blob: Some("blob".into()),
            }],
        },
    );
    let mut comments: Vec<_> = (1..25)
        .map(|i| comment(&format!("comment{i}"), &format!("issuecomment-{i}")))
        .collect();
    comments.push(comment("conversation", "issuecomment-100"));
    gh.set_conversation(
        &repo(),
        7,
        Conversation {
            comments,
            truncated: true,
        },
    );
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(600.)));
    for (suffix, tab, marked) in [
        ("/files#discussion_r200", PrTab::Files, Some("reply")),
        ("#discussion_r300", PrTab::Threads, Some("old")),
        (
            "#issuecomment-100",
            PrTab::Description,
            Some("conversation"),
        ),
        ("/files?view=split#discussion_r999", PrTab::Threads, None),
        ("?view=all#issuecomment-101", PrTab::Description, None),
        (
            "#issuecomment-110",
            PrTab::Description,
            Some("newly-posted"),
        ),
    ] {
        if marked == Some("newly-posted") {
            gh.set_conversation(
                &repo(),
                7,
                Conversation {
                    comments: vec![comment("newly-posted", "issuecomment-110")],
                    truncated: false,
                },
            );
        }
        let url = format!("https://github.com/o/r/pull/7{suffix}");
        vcx.update(|window, cx| {
            f.ws.update(cx, |ws, cx| {
                let (repo, address) = ws.resolve_pr_address(&url, cx).unwrap();
                ws.open_pr_by_address(repo, address, PrDestination::Page, window, cx);
            })
        });
        vcx.run_until_parked();
        f.ws.read_with(&vcx, |ws, _| {
            let state = ws.pages.pull_requests.detail().unwrap();
            assert_eq!(state.tab(), tab, "{suffix}");
            if let Some(node) = marked {
                assert_eq!(state.comment_mark(node), CommentMark::Highlight, "{suffix}");
            } else {
                let view = ws.pages.pull_requests.detail_snapshot.as_ref().unwrap();
                assert_eq!(view.missing_comment_url.as_ref(), Some(&url), "{suffix}");
            }
        });
        if marked.is_some() {
            let linked = vcx
                .debug_bounds("pr-linked-comment")
                .expect("comment marked");
            let viewport = f.ws.read_with(&vcx, |ws, _| {
                ws.pages.pull_requests.detail().unwrap().scroll().bounds()
            });
            assert!(
                linked.top() >= viewport.top() - gpui::px(1.) && linked.top() < viewport.bottom(),
                "{suffix}: {linked:?} in {viewport:?}"
            );
        } else {
            assert!(
                vcx.debug_bounds("pr-linked-comment-github").is_some(),
                "{suffix}"
            );
        }
    }
}

#[test]
fn a_failed_read_with_a_retry_out_still_waits_for_the_link() {
    use super::super::read_slot::{ReadSlot, Tickets};
    let mut slot: ReadSlot<()> = ReadSlot::settled(Load::Failed("HTTP 502".into()));
    assert!(matches!(missing_unless_waiting(&slot), Located::Missing));
    slot.reread(Tickets::default().issue());
    assert!(
        matches!(missing_unless_waiting(&slot), Located::Waiting),
        "the retry answers it"
    );
}
