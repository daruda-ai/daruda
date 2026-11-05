use super::ComposerLayout;

#[test]
fn large_rows_leave_room_for_controls() {
    let layout = ComposerLayout::new(500.0, 88.0, 144.0, 3);
    assert_eq!(layout.max_rows, 2);
    assert_eq!(layout.height(3), 376.0);
}

#[test]
fn strips_reduce_capacity_and_releasing_them_restores_it() {
    let bare = ComposerLayout::new(500.0, 88.0, 36.0, 20);
    let occupied = ComposerLayout::new(500.0, 88.0 + 32.0 + 160.0 + 64.0, 36.0, 20);
    assert_eq!(bare.max_rows, 11);
    assert_eq!(occupied.max_rows, 4);
    assert_eq!(occupied.height(20), 488.0);
    assert_eq!(bare.height(20), 484.0);
    let largest = ComposerLayout::new(500.0, 88.0 + 32.0 + 160.0 + 64.0, 144.0, 20);
    assert_eq!(largest.max_rows, 1);
    assert_eq!(largest.height(20), 488.0);
}

#[test]
fn configured_rows_remain_an_upper_bound() {
    let layout = ComposerLayout::new(500.0, 88.0, 20.0, 3);
    assert_eq!(layout.max_rows, 3);
    assert_eq!(layout.height(1), 108.0);
    assert_eq!(layout.height(20), 148.0);
    let expanded = ComposerLayout::new(500.0, 88.0, 20.0, 20);
    assert_eq!(expanded.max_rows, 20);
    assert_eq!(expanded.height(20), 488.0);
}
