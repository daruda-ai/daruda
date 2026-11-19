use daruda_github::{CheckRollup, PrListItem, PrState, PrSummary, RepoId};

use super::*;

fn repo() -> RepoId {
    RepoId::new("github.com", "o", "r")
}

fn detail(id: u64, number: u64) -> PrDetailState {
    PrDetailState::new(
        PrDetailId(id),
        PrListItem {
            repo: repo(),
            pr: PrSummary {
                number,
                url: String::new(),
                title: String::new(),
                state: PrState::Open,
                head_branch: String::new(),
                cross_repo: false,
                checks: CheckRollup::None,
                failed: Vec::new(),
                review: None,
                updated_at: String::new(),
            },
            author: None,
            head_owner: None,
            base_branch: String::new(),
            comments: 0,
        },
    )
}

fn ended(detail: Option<PrDetailState>) -> Option<u64> {
    detail.map(|detail| detail.id().0)
}

#[test]
fn a_detail_the_page_stops_showing_ends_unless_a_pane_holds_it() {
    let mut details = PrDetails::default();
    let first = details.open(detail(1, 480));
    assert_eq!(ended(details.show_on_page(Some(first))), None);
    assert_eq!(details.on_page().map(PrDetailState::id), Some(first));
    assert_eq!(details.of(&repo(), 480), Some(first));

    // Another replaces it on the page: the first ends.
    let second = details.open(detail(2, 481));
    assert_eq!(ended(details.show_on_page(Some(second))), Some(1));
    assert_eq!(details.of(&repo(), 480), None);

    // Held by a pane, it outlives the page showing it.
    details.hold(second);
    assert_eq!(ended(details.show_on_page(None)), None);
    assert!(details.on_page().is_none());
    assert!(details.get(second).is_some());
    assert_eq!(ended(details.release(second)), Some(2));
    assert_eq!(details.len(), 0);
}

#[test]
fn a_detail_ends_with_the_last_thing_showing_it() {
    let mut details = PrDetails::default();
    let id = details.open(detail(1, 480));
    details.hold(id);
    details.hold(id);
    details.show_on_page(Some(id));

    assert_eq!(ended(details.release(id)), None, "a pane and the page");
    assert_eq!(ended(details.release(id)), None, "the page");
    assert_eq!(ended(details.release(id)), None, "no pane holds it");
    assert_eq!(ended(details.show_on_page(None)), Some(1));
}

#[test]
fn showing_the_same_detail_again_or_one_not_open_ends_nothing() {
    let mut details = PrDetails::default();
    let id = details.open(detail(1, 480));
    details.show_on_page(Some(id));
    assert_eq!(ended(details.show_on_page(Some(id))), None);
    assert!(details.on_page().is_some());
    // An id nothing is open under puts nothing on the page.
    assert_eq!(ended(details.show_on_page(Some(PrDetailId(9)))), Some(1));
    assert!(details.on_page().is_none());
    details.hold(PrDetailId(9));
    assert_eq!(ended(details.release(PrDetailId(9))), None);
}
