//! A patch cut to a byte budget shared between its files, so one
//! generated file cannot leave the others unseen.

/// The least of the budget one file's part is worth showing for: with more
/// files than the budget has of these, the later ones are only counted.
const PART_BYTES: usize = 400;
/// Room kept for one note of what was left out.
const NOTE_BYTES: usize = 80;

/// `patch` within `budget` bytes, notes included. Each file's part gets
/// an equal share; what a small file leaves unused goes to the larger
/// ones. A part over its share is cut at a line end, with a note of how
/// much is left out. With more files than the budget can show
/// [`PART_BYTES`] of each, the later ones are counted in one note.
pub(super) fn cut_patch(patch: &str, budget: usize) -> String {
    if patch.len() <= budget {
        return patch.to_owned();
    }
    let all = file_parts(patch);
    let shown = all.len().min(budget / PART_BYTES).max(1);
    let (parts, hidden) = all.split_at(shown);
    // Smallest first: each takes at most an equal share of what is left
    // once every note that may be written has its room.
    let mut order: Vec<usize> = (0..parts.len()).collect();
    order.sort_by_key(|&ix| parts[ix].len());
    let mut shares = vec![0; parts.len()];
    let mut left = budget.saturating_sub((parts.len() + 1) * NOTE_BYTES);
    for (taken, &ix) in order.iter().enumerate() {
        let share = (left / (parts.len() - taken)).min(parts[ix].len());
        shares[ix] = share;
        left -= share;
    }
    let mut out = String::with_capacity(budget);
    for (part, share) in parts.iter().zip(shares) {
        if share == part.len() {
            out.push_str(part);
            continue;
        }
        let mut kept = &part[..line_end_within(part, share)];
        if kept.is_empty() {
            // A first line longer than the share: as much of it as fits,
            // so the note still says which file it is about.
            kept = &part[..boundary_within(part, share)];
            out.push_str(kept);
            out.push('\n');
        } else {
            out.push_str(kept);
        }
        out.push_str(&format!(
            "[… {} bytes of this file's patch omitted …]\n",
            part.len() - kept.len()
        ));
    }
    if !hidden.is_empty() {
        out.push_str(&format!(
            "[… {} more files' patches omitted ({} bytes) …]\n",
            hidden.len(),
            hidden.iter().map(|part| part.len()).sum::<usize>()
        ));
    }
    out
}

/// `patch` split where each file's part starts (`diff --git`), every byte
/// kept.
fn file_parts(patch: &str) -> Vec<&str> {
    const START: &str = "diff --git ";
    let mut starts: Vec<usize> = patch
        .match_indices(START)
        .map(|(at, _)| at)
        .filter(|&at| at == 0 || patch.as_bytes()[at - 1] == b'\n')
        .collect();
    if starts.first() != Some(&0) {
        starts.insert(0, 0);
    }
    let ends = starts.iter().skip(1).copied().chain([patch.len()]);
    starts
        .iter()
        .zip(ends)
        .map(|(&start, end)| &patch[start..end])
        .collect()
}

/// The longest prefix of `text` within `limit` bytes that ends after a
/// newline; nothing when its first line is longer.
fn line_end_within(text: &str, limit: usize) -> usize {
    text[..boundary_within(text, limit)]
        .rfind('\n')
        .map_or(0, |at| at + 1)
}

/// The longest prefix of `text` within `limit` bytes that ends between
/// characters.
fn boundary_within(text: &str, limit: usize) -> usize {
    let mut end = limit.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    end
}

#[cfg(test)]
mod tests;
