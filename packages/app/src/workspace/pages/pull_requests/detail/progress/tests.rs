use daruda_github::RepoId;

use super::*;

fn key(number: u64) -> PrKey {
    (RepoId::new("github.com", "o", "n"), number)
}

#[test]
fn a_mark_holds_only_for_the_content_it_was_made_at() {
    let mut progress = ReviewProgress::default();
    let pr = key(7);
    assert_eq!(progress.viewed(&pr, "a.rs", Some("b1")), Viewed::No);
    progress.toggle_viewed(&pr, "a.rs", Some("b1"));
    // (label, blob now, expected)
    let cases = [
        ("the same content", Some("b1"), Viewed::Yes),
        ("new content", Some("b2"), Viewed::Changed),
        ("content GitHub no longer identifies", None, Viewed::Changed),
    ];
    for (label, blob, expected) in cases {
        assert_eq!(progress.viewed(&pr, "a.rs", blob), expected, "{label}");
    }
    assert_eq!(
        progress.viewed(&pr, "b.rs", Some("b1")),
        Viewed::No,
        "per file"
    );
    assert_eq!(
        progress.viewed(&key(8), "a.rs", Some("b1")),
        Viewed::No,
        "per pull request"
    );
}

#[test]
fn toggling_clears_a_holding_mark_and_renews_a_changed_one() {
    let mut progress = ReviewProgress::default();
    let pr = key(7);
    progress.toggle_viewed(&pr, "a.rs", Some("b1"));
    progress.toggle_viewed(&pr, "a.rs", Some("b2"));
    assert_eq!(
        progress.viewed(&pr, "a.rs", Some("b2")),
        Viewed::Yes,
        "renewed at the new content"
    );
    progress.toggle_viewed(&pr, "a.rs", Some("b2"));
    assert_eq!(progress.viewed(&pr, "a.rs", Some("b2")), Viewed::No);
    progress.toggle_viewed(&pr, "a.rs", None);
    assert_eq!(
        progress.viewed(&pr, "a.rs", None),
        Viewed::No,
        "unidentified content is not marked"
    );
}

#[test]
fn the_least_recently_touched_pull_request_goes_first() {
    let mut progress = ReviewProgress::default();
    let place = Place {
        tab: PrTab::Files,
        file: None,
        offset: (0.0, -40.0),
    };
    for number in 0..KEPT as u64 {
        progress.leave(&key(number), place.clone());
    }
    progress.toggle_viewed(&key(0), "a.rs", Some("b"));
    progress.leave(&key(1_000), place.clone());
    assert!(progress.place(&key(0)).is_some(), "touched again, kept");
    assert!(
        progress.place(&key(1)).is_none(),
        "the oldest untouched went"
    );
    assert_eq!(progress.place(&key(1_000)), Some(&place));
}
