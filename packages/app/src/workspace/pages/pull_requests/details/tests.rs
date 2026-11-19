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

fn ended(details: &mut PrDetails) -> Vec<u64> {
    details
        .take_ended()
        .iter()
        .map(|detail| detail.id().0)
        .collect()
}

#[test]
fn a_detail_the_page_stops_showing_ends_unless_a_pane_holds_it() {
    let mut details = PrDetails::default();
    let first = details.open(detail(1, 480));
    let first_id = first.id();
    details.show_on_page(Some(first));
    assert_eq!(details.on_page().map(PrDetailState::id), Some(first_id));
    assert_eq!(details.of(&repo(), 480), Some(first_id));
    assert!(ended(&mut details).is_empty());

    // Another replaces it on the page: the first is over, and not found.
    let second = details.open(detail(2, 481));
    let pane = second.clone();
    details.show_on_page(Some(second));
    assert_eq!(details.of(&repo(), 480), None);
    assert!(details.get(first_id).is_none());
    assert_eq!(ended(&mut details), [1]);

    // Held by a pane, it outlives the page showing it.
    details.show_on_page(None);
    assert!(details.on_page().is_none());
    assert!(details.get(pane.id()).is_some());
    assert!(ended(&mut details).is_empty());
    drop(pane);
    assert_eq!(details.len(), 0);
    assert_eq!(ended(&mut details), [2]);
}

#[test]
fn a_detail_ends_with_the_last_hold_however_it_is_dropped() {
    let mut details = PrDetails::default();
    let first = details.open(detail(1, 480));
    let id = first.id();
    let second = details.hold(id).expect("it is open");
    let third = second.clone();
    details.show_on_page(Some(first));

    drop(second);
    details.show_on_page(None);
    assert_eq!(details.len(), 1, "one hold is left");
    // A pane torn down with its runtime drops its hold with it.
    let runtime = vec![third];
    drop(runtime);
    assert_eq!(details.len(), 0);
    assert!(
        details.hold(id).is_none(),
        "an ended detail takes no new hold"
    );
    assert_eq!(ended(&mut details), [1]);
    assert!(ended(&mut details).is_empty(), "taken once");
}

#[test]
fn a_hold_is_only_on_a_detail_that_is_open() {
    let mut details = PrDetails::default();
    assert!(details.hold(PrDetailId(9)).is_none());
    let hold = details.open(detail(1, 480));
    assert_eq!(details.hold(hold.id()).map(|h| h.id()), Some(hold.id()));
    assert!(details.get_mut(hold.id()).is_some());
}
