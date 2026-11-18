//! Tying worktrees to pull requests by hand: untying one its branch name
//! found survives a refresh and a restart, history keeps it, restoring
//! lets the name find it again, and a worktree can be tied to another.

use daruda_github::fake::{FakeGh, GhOp};
use daruda_store::pr_links::{LinkSource, load_pr_links_in};
use daruda_store::project::LaneRef;
use gpui::TestAppContext;

use super::GitFixture;
use super::pull_request_detail::{item, open, repo, window};
use crate::github::LinkStore;
use crate::github::links::Source;

/// The lane's pull request number, as the Git panel reads it.
fn linked(f: &GitFixture, lane: LaneRef, cx: &mut TestAppContext) -> Option<u64> {
    f.ws.read_with(cx, |ws, _| {
        ws.lane_scoped
            .get(&lane)
            .and_then(|state| state.pr.link().map(|link| link.pr.number))
    })
}

fn source(f: &GitFixture, lane: LaneRef, cx: &mut TestAppContext) -> Option<Source> {
    f.ws.read_with(cx, |ws, _| {
        ws.lane_scoped.get(&lane).and_then(|s| s.link_source())
    })
}

/// Polls answered, at once.
fn poll(cx: &mut TestAppContext) {
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(31));
    cx.update(crate::github::PrPoller::tick);
    cx.run_until_parked();
}

fn with_links(cx: &mut TestAppContext) -> (tempfile::TempDir, GitFixture, FakeGh, LaneRef) {
    let dir = tempfile::tempdir().unwrap();
    cx.update(|cx| LinkStore::install_at(dir.path().to_owned(), cx));
    let (f, gh, lane) = window(cx);
    poll(cx);
    (dir, f, gh, lane)
}

#[gpui::test]
fn an_untied_pull_request_stays_untied_until_restored(cx: &mut TestAppContext) {
    let (dir, f, _gh, lane) = with_links(cx);
    assert_eq!(linked(&f, lane, cx), Some(7), "found by its branch name");
    assert_eq!(source(&f, lane, cx), Some(Source::Branch));

    f.ws.update(cx, |ws, cx| ws.unlink_pr_lane(lane, cx));
    poll(cx);
    assert_eq!(linked(&f, lane, cx), None, "a poll does not bring it back");
    assert_eq!(
        f.ws.read_with(cx, |ws, cx| ws.pr_strip(cx)),
        crate::workspace::github_ops::strip::PrStrip::Untied { number: 7 },
        "the Git panel offers tying it again, not creating another"
    );

    // A restart: the store read again from disk, the lane mirrored again.
    cx.update(LinkStore::flush);
    cx.update(|cx| LinkStore::install_at(dir.path().to_owned(), cx));
    f.ws.update(cx, |ws, cx| ws.sync_lane_links(lane, cx));
    poll(cx);
    assert_eq!(linked(&f, lane, cx), None, "nor does a restart");

    let saved = load_pr_links_in(dir.path()).unwrap();
    let history = &saved.worktrees[0].history;
    assert_eq!(history.len(), 1, "{history:?}");
    assert_eq!(
        (
            history[0].pr.number,
            history[0].source,
            history[0].unlinked_at.is_some()
        ),
        (7, LinkSource::Branch, true),
        "history keeps it, untied"
    );

    // The detail of #7 offers the worktree back.
    open(&f, item(7, "feat"), cx);
    cx.run_until_parked();
    let unlinked = f.ws.update(cx, |ws, cx| {
        ws.stage_pr_page(cx);
        ws.pages
            .pull_requests
            .detail_snapshot
            .as_ref()
            .and_then(|view| match &view.body {
                crate::workspace::pages::pull_requests::detail::view::DetailBody::Ready(ready) => {
                    Some(ready.side.link.unlinked.len())
                }
                _ => None,
            })
    });
    assert_eq!(unlinked, Some(1));

    f.ws.update(cx, |ws, cx| ws.restore_pr_lane(lane, cx));
    poll(cx);
    assert_eq!(linked(&f, lane, cx), Some(7), "found by name again");
}

/// Untied and then merged: the branch has no pull request to tie again,
/// so the Git panel offers creating one.
#[gpui::test]
fn an_untied_pull_request_merged_since_offers_creating(cx: &mut TestAppContext) {
    let (_dir, f, gh, lane) = with_links(cx);
    f.ws.update(cx, |ws, cx| ws.unlink_pr_lane(lane, cx));
    poll(cx);
    let mut merged = item(7, "feat").pr;
    merged.state = daruda_github::PrState::Merged;
    gh.set_prs(&repo(), vec![merged]);
    poll(cx);
    assert_eq!(
        f.ws.read_with(cx, |ws, cx| ws.pr_strip(cx)),
        crate::workspace::github_ops::strip::PrStrip::NoPr
    );
}

#[gpui::test]
fn a_worktree_can_be_tied_to_another_pull_request(cx: &mut TestAppContext) {
    let (_dir, f, gh, lane) = with_links(cx);
    let mut other = item(8, "other").pr;
    other.title = "Other".into();
    gh.set_prs(&repo(), vec![item(7, "feat").pr, other]);

    f.ws.update(cx, |ws, cx| ws.link_pr_to_lane(lane, &repo(), 8, cx));
    poll(cx);
    assert_eq!(linked(&f, lane, cx), Some(8), "asked for by number");
    assert_eq!(source(&f, lane, cx), Some(Source::Chosen));
    assert!(gh.count(GhOp::PrsForBranches) > 0);

    // Untied: its branch name finds #7 again.
    f.ws.update(cx, |ws, cx| ws.unlink_pr_lane(lane, cx));
    poll(cx);
    assert_eq!(linked(&f, lane, cx), Some(7));
}

/// Chosen, and also the one its branch name finds: untied, it stays untied.
#[gpui::test]
fn untying_a_chosen_pull_request_the_name_also_finds_keeps_it_untied(cx: &mut TestAppContext) {
    let (_dir, f, _gh, lane) = with_links(cx);
    f.ws.update(cx, |ws, cx| ws.link_pr_to_lane(lane, &repo(), 7, cx));
    poll(cx);
    assert_eq!(source(&f, lane, cx), Some(Source::Chosen));
    f.ws.update(cx, |ws, cx| ws.unlink_pr_lane(lane, cx));
    poll(cx);
    assert_eq!(linked(&f, lane, cx), None);
}
