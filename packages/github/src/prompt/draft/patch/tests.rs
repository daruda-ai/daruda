use super::*;

fn part(name: &str, lines: usize) -> String {
    let mut out = format!("diff --git a/{name} b/{name}\n");
    for n in 0..lines {
        out.push_str(&format!("+{name} line {n}\n"));
    }
    out
}

/// One generated file does not leave the small ones unseen: they stay
/// whole and only the large one is cut, at a line end, with what is left
/// out counted.
#[test]
fn a_large_file_is_cut_and_the_small_ones_stay_whole() {
    let (small_a, large, small_b) = (part("a.rs", 3), part("gen.lock", 2_000), part("b.rs", 3));
    let patch = format!("{small_a}{large}{small_b}");
    let budget = 2_000;
    let cut = cut_patch(&patch, budget);
    assert!(cut.contains(&small_a) && cut.contains(&small_b), "{cut}");
    assert!(cut.contains("+gen.lock line 0\n"), "its start is kept");
    assert!(!cut.contains("+gen.lock line 1999"), "its end is not");
    let note = cut
        .lines()
        .find(|line| line.starts_with("[… "))
        .expect("what was left out is said");
    let omitted: usize = note
        .split_whitespace()
        .nth(1)
        .and_then(|n| n.parse().ok())
        .expect("a byte count");
    let kept = cut.len() - note.len() - 1;
    assert_eq!(kept + omitted, patch.len(), "every byte kept or counted");
    assert!(kept <= budget, "{kept}");
    for line in cut.lines() {
        assert!(
            line.starts_with("diff --git ") || line.starts_with('+') || line.starts_with("[… "),
            "cut inside a line: {line:?}"
        );
    }
}

/// More files than the budget can show a part of: the result still fits,
/// the first ones are named and the rest are counted in one note.
#[test]
fn more_files_than_the_budget_shows_are_counted() {
    let files = 500;
    let patch: String = (0..files).map(|n| part(&format!("f{n}.rs"), 20)).collect();
    let budget = 4_000;
    let cut = cut_patch(&patch, budget);
    assert!(cut.len() <= budget, "{} over {budget}", cut.len());
    let shown = cut.matches("diff --git ").count();
    assert_eq!(shown, budget / PART_BYTES);
    assert!(cut.starts_with("diff --git a/f0.rs b/f0.rs\n"), "{cut}");
    assert!(
        cut.contains(&format!(
            "[… {} more files' patches omitted (",
            files - shown
        )),
        "{cut}"
    );
}

/// A first line longer than a file's share is cut, not dropped: the note
/// after it still follows the file's name.
#[test]
fn a_header_longer_than_its_share_still_names_the_file() {
    let name = "n".repeat(600);
    let patch = format!("{}{}", part(&name, 50), part("b.rs", 50));
    let cut = cut_patch(&patch, 1_000);
    assert!(cut.starts_with("diff --git a/nnnn"), "{cut}");
    assert!(cut.len() <= 1_000, "{}", cut.len());
}

#[test]
fn a_patch_within_the_budget_is_quoted_as_it_is() {
    let patch = part("a.rs", 3);
    assert_eq!(cut_patch(&patch, patch.len()), patch);
}

/// A cut never lands inside a character.
#[test]
fn a_cut_keeps_whole_characters() {
    let patch = format!(
        "diff --git a/k b/k\n+{}\n+{}\n",
        "가".repeat(40),
        "나".repeat(40)
    );
    // From where the first line fits beside the room kept for notes.
    for budget in 200..patch.len() {
        let cut = cut_patch(&patch, budget);
        assert!(cut.starts_with("diff --git a/k b/k\n"), "{budget}: {cut}");
    }
}
