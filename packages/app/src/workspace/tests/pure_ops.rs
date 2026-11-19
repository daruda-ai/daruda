//! Synchronous unit tests for pure helper functions — no GPUI, no
//! async. Paired file with `tests/mod.rs`, which holds the
//! `TestAppContext`-based lifecycle tests.

use crate::workspace::main_area::pane_tree::{
    PaneId, PaneLayout, SplitDirection, adjust_divider, cleanup_after_remove, collect_pane_rects,
    collect_pane_sizes, insert_split_at, remove_pane_from_layout,
};

fn split(dir: SplitDirection, children: Vec<PaneLayout>) -> PaneLayout {
    PaneLayout::new_split(dir, children)
}

fn leaf(id: PaneId) -> PaneLayout {
    PaneLayout::Pane(id)
}

// ---- PaneLayout unit tests ----

#[test]
fn test_pane_layout_contains() {
    let layout = split(
        SplitDirection::Horizontal,
        vec![leaf(PaneId(1)), leaf(PaneId(2))],
    );
    assert!(layout.contains(PaneId(1)));
    assert!(layout.contains(PaneId(2)));
    assert!(!layout.contains(PaneId(3)));
}

#[test]
fn test_pane_layout_pane_ids() {
    let layout = split(
        SplitDirection::Horizontal,
        vec![
            leaf(PaneId(10)),
            split(
                SplitDirection::Vertical,
                vec![leaf(PaneId(20)), leaf(PaneId(30))],
            ),
        ],
    );
    assert_eq!(layout.pane_ids(), vec![10, 20, 30]);
    assert_eq!(layout.leaf_count(), 3);
    assert_eq!(layout.first_leaf(), PaneId(10));
}

#[test]
fn test_pane_layout_next_prev() {
    let layout = split(
        SplitDirection::Horizontal,
        vec![leaf(PaneId(1)), leaf(PaneId(2))],
    );
    assert_eq!(layout.next_pane(PaneId(1)), Some(2.into()));
    assert_eq!(layout.next_pane(PaneId(2)), Some(1.into()));
    assert_eq!(layout.prev_pane(PaneId(2)), Some(1.into()));
    assert_eq!(layout.prev_pane(PaneId(1)), Some(2.into()));
}

#[test]
fn test_insert_split_at_root_leaf() {
    let mut layout = leaf(PaneId(1));
    assert!(insert_split_at(
        &mut layout,
        1.into(),
        SplitDirection::Horizontal,
        2.into(),
        false
    ));
    assert_eq!(layout.pane_ids(), vec![1, 2]);
    assert!(matches!(
        layout,
        PaneLayout::Split {
            direction: SplitDirection::Horizontal,
            ..
        }
    ));
}

#[test]
fn test_insert_split_same_direction_splices_into_parent() {
    // [1 | 2] horizontal, split 2 horizontally → [1 | 2 | 3]
    let mut layout = split(
        SplitDirection::Horizontal,
        vec![leaf(PaneId(1)), leaf(PaneId(2))],
    );
    assert!(insert_split_at(
        &mut layout,
        2.into(),
        SplitDirection::Horizontal,
        3.into(),
        false
    ));
    assert_eq!(layout.pane_ids(), vec![1, 2, 3]);
    if let PaneLayout::Split { children, .. } = &layout {
        assert_eq!(children.len(), 3);
    } else {
        panic!("expected Split");
    }
}

#[test]
fn test_insert_split_opposite_direction_creates_nested() {
    // [1 | 2] horizontal, split 2 vertically → [1 | [2 / 3]]
    let mut layout = split(
        SplitDirection::Horizontal,
        vec![leaf(PaneId(1)), leaf(PaneId(2))],
    );
    assert!(insert_split_at(
        &mut layout,
        2.into(),
        SplitDirection::Vertical,
        3.into(),
        false
    ));
    assert_eq!(layout.pane_ids(), vec![1, 2, 3]);
    if let PaneLayout::Split {
        direction,
        children,
        ..
    } = &layout
    {
        assert_eq!(*direction, SplitDirection::Horizontal);
        assert_eq!(children.len(), 2);
        assert!(matches!(
            &children[1],
            PaneLayout::Split {
                direction: SplitDirection::Vertical,
                ..
            }
        ));
    } else {
        panic!("expected Split");
    }
}

#[test]
fn test_remove_pane_collapses_split() {
    let mut layout = split(
        SplitDirection::Horizontal,
        vec![leaf(PaneId(1)), leaf(PaneId(2))],
    );
    assert!(remove_pane_from_layout(&mut layout, PaneId(1)));
    assert!(matches!(layout, PaneLayout::Pane(PaneId(2))));
}

#[test]
fn test_remove_pane_nested_promotes() {
    // [A | [B / C]] → remove B → [A | C]
    let mut layout = split(
        SplitDirection::Horizontal,
        vec![
            leaf(PaneId(1)),
            split(
                SplitDirection::Vertical,
                vec![leaf(PaneId(2)), leaf(PaneId(3))],
            ),
        ],
    );
    assert!(remove_pane_from_layout(&mut layout, PaneId(2)));
    assert_eq!(layout.pane_ids(), vec![1, 3]);
    assert_eq!(layout.leaf_count(), 2);
    assert!(matches!(
        layout,
        PaneLayout::Split {
            direction: SplitDirection::Horizontal,
            ..
        }
    ));
}

#[test]
fn test_cleanup_flattens_same_direction_nested() {
    // Manually construct [1 | [2 | 3]] (both Horizontal) and expect flatten.
    let inner = split(
        SplitDirection::Horizontal,
        vec![leaf(PaneId(2)), leaf(PaneId(3))],
    );
    let mut layout = split(SplitDirection::Horizontal, vec![leaf(PaneId(1)), inner]);
    cleanup_after_remove(&mut layout);
    if let PaneLayout::Split { children, .. } = &layout {
        assert_eq!(children.len(), 3);
        assert!(matches!(children[0], PaneLayout::Pane(PaneId(1))));
        assert!(matches!(children[1], PaneLayout::Pane(PaneId(2))));
        assert!(matches!(children[2], PaneLayout::Pane(PaneId(3))));
    } else {
        panic!("expected Split");
    }
}

#[test]
fn test_collect_pane_sizes_single() {
    let layout = leaf(PaneId(1));
    let mut sizes = Vec::new();
    collect_pane_sizes(&layout, 800.0, 600.0, &mut sizes);
    assert_eq!(sizes.len(), 1);
    assert_eq!(sizes[0], (1.into(), 800.0, 600.0));
}

#[test]
fn test_collect_pane_sizes_h_split() {
    let layout = split(
        SplitDirection::Horizontal,
        vec![leaf(PaneId(1)), leaf(PaneId(2))],
    );
    let mut sizes = Vec::new();
    collect_pane_sizes(&layout, 801.0, 600.0, &mut sizes);
    assert_eq!(sizes.len(), 2);
    assert_eq!(sizes[0], (1.into(), 400.0, 600.0));
    assert_eq!(sizes[1], (2.into(), 400.0, 600.0));
}

#[test]
fn test_collect_pane_sizes_v_split() {
    let layout = split(
        SplitDirection::Vertical,
        vec![leaf(PaneId(1)), leaf(PaneId(2))],
    );
    let mut sizes = Vec::new();
    collect_pane_sizes(&layout, 800.0, 601.0, &mut sizes);
    assert_eq!(sizes.len(), 2);
    assert_eq!(sizes[0], (1.into(), 800.0, 300.0));
    assert_eq!(sizes[1], (2.into(), 800.0, 300.0));
}

#[test]
fn test_collect_pane_rects_nested() {
    // Horizontal: [1 | V[2/3]] at 800×600 (divider 1px) → w1=399.5, w_right=399.5
    let layout = split(
        SplitDirection::Horizontal,
        vec![
            leaf(PaneId(1)),
            split(
                SplitDirection::Vertical,
                vec![leaf(PaneId(2)), leaf(PaneId(3))],
            ),
        ],
    );
    let mut rects = Vec::new();
    collect_pane_rects(&layout, 0.0, 0.0, 801.0, 601.0, &mut rects);
    assert_eq!(rects.len(), 3);
    assert!((rects[0].w - 400.0).abs() < 0.01);
    assert!((rects[0].h - 601.0).abs() < 0.01);
    assert!((rects[1].h - 300.0).abs() < 0.01);
    assert!((rects[2].h - 300.0).abs() < 0.01);
    assert!((rects[1].x - rects[2].x).abs() < 0.01);
}

#[test]
fn test_adjust_divider_changes_ratios() {
    let mut layout = split(
        SplitDirection::Horizontal,
        vec![leaf(PaneId(1)), leaf(PaneId(2))],
    );
    assert!(adjust_divider(&mut layout, PaneId(1), 0.2));
    if let PaneLayout::Split { ratios, .. } = &layout {
        assert!((ratios[0] - 0.7).abs() < 1e-4);
        assert!((ratios[1] - 0.3).abs() < 1e-4);
    } else {
        panic!("expected Split");
    }
}

// ---- normalize_ratios ----

#[test]
fn test_normalize_ratios_normalizes_sum() {
    // Existing ratios sum to 2.0 → each should be halved to sum 1.0.
    let r = super::super::persistence::normalize_ratios(&[0.6, 1.4], 2);
    assert!((r.iter().sum::<f32>() - 1.0).abs() < 1e-4);
    assert!((r[0] - 0.3).abs() < 1e-4);
    assert!((r[1] - 0.7).abs() < 1e-4);
}

#[test]
fn test_normalize_ratios_length_mismatch_falls_back_to_equal() {
    // len != expected → equal distribution.
    let r = super::super::persistence::normalize_ratios(&[0.5], 3);
    assert_eq!(r.len(), 3);
    for v in &r {
        assert!((*v - 1.0 / 3.0).abs() < 1e-4);
    }
}

#[test]
fn test_normalize_ratios_zero_sum_falls_back_to_equal() {
    let r = super::super::persistence::normalize_ratios(&[0.0, 0.0], 2);
    assert_eq!(r, vec![0.5, 0.5]);
}

// ---- resolve_default_cwd ----
//
// New-pane cwd resolver. Verifies that the active lane's path
// outranks the main project root so `Cmd+T` (and the very first
// `add_tab` at startup) lands inside the lane the user picked,
// never at the umbrella project root.

mod resolve_default_cwd {
    use crate::workspace::main_area::pane::{CwdCandidates, resolve_default_cwd};
    use std::path::PathBuf;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    /// Standard lane-aware setup — focused pane is inside the
    /// active lane. Tests override fields they care about.
    fn typical_candidates() -> CwdCandidates {
        CwdCandidates {
            focused_pane: Some(p("/Users/x/proj-feat-a/src")),
            active_lane: Some(p("/Users/x/proj-feat-a")),
            project_root: Some(p("/Users/x/proj")),
        }
    }

    #[test]
    fn focused_pane_cwd_wins() {
        let got = resolve_default_cwd(typical_candidates());
        assert_eq!(got, Some(p("/Users/x/proj-feat-a/src")));
    }

    #[test]
    fn active_worktree_path_wins_over_project_root_at_startup() {
        // Fresh `new_with_project`: no panes yet, focused-pane cwd
        // is None. The new tab MUST spawn at the lane path,
        // not at the main repo root.
        let got = resolve_default_cwd(CwdCandidates {
            focused_pane: None,
            ..typical_candidates()
        });
        assert_eq!(got, Some(p("/Users/x/proj-feat-a")));
    }

    #[test]
    fn project_root_is_last_resort() {
        // No lane info available (legacy / non-lane
        // workspace). Falls through to project root.
        let got = resolve_default_cwd(CwdCandidates {
            project_root: Some(p("/Users/x/proj")),
            ..Default::default()
        });
        assert_eq!(got, Some(p("/Users/x/proj")));
    }

    #[test]
    fn returns_none_when_no_candidates() {
        // Project-less Workspace, no lanes, no focused pane.
        // The PTY then falls back to the parent process's cwd.
        let got = resolve_default_cwd(CwdCandidates::default());
        assert_eq!(got, None);
    }

    #[test]
    fn focused_cwd_in_worktree_kept_verbatim() {
        // A pane that has actually published OSC 7 keeps its
        // reported cwd verbatim — even if it has navigated into a
        // subdirectory of the lane, we don't reset to the
        // lane root.
        let got = resolve_default_cwd(CwdCandidates {
            focused_pane: Some(p("/Users/x/proj-feat-a/deep/nested/dir")),
            ..typical_candidates()
        });
        assert_eq!(got, Some(p("/Users/x/proj-feat-a/deep/nested/dir")));
    }
}
