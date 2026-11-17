use super::*;

#[test]
fn sides_map_to_the_draft_and_back() {
    for side in [Side::Old, Side::New] {
        assert_eq!(view_side(draft_side(side)), side);
    }
    assert_eq!(
        draft_side(Side::New),
        DraftSide::Right,
        "GitHub's head side"
    );
}
