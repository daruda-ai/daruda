use daruda_github::{FileChange, PR_FILES_PER_PAGE, PrFile, PrFilesPage};

use super::*;
use crate::workspace::pages::pull_requests::detail::progress::{PlaceFile, Viewed};
use crate::workspace::pages::pull_requests::detail::read_slot::Tickets;

fn file(path: &str, change: FileChange, patch: Option<&str>) -> PrFile {
    PrFile {
        path: path.into(),
        change,
        additions: 2,
        deletions: 1,
        patch: patch.map(str::to_owned),
        blob: Some(format!("{path}@{}", patch.unwrap_or_default())),
    }
}

fn edit(path: &str) -> PrFile {
    file(path, FileChange::Modified, Some("@@ -1 +1 @@\n-x\n+y"))
}

fn full_page(from: usize, patch: &str) -> Vec<PrFile> {
    (from..from + PR_FILES_PER_PAGE)
        .map(|n| file(&format!("f{n}"), FileChange::Modified, Some(patch)))
        .collect()
}

/// A state reading page 1, and the tickets it was issued from.
/// No file marked viewed.
fn unmarked(_: &PrFile) -> Viewed {
    Viewed::No
}

fn reading() -> (FilesState, Tickets, ReadTicket) {
    let mut tickets = Tickets::default();
    let ticket = tickets.issue();
    (FilesState::reading(ticket), tickets, ticket)
}

#[test]
fn pages_are_read_in_turn_until_the_last() {
    let (mut state, mut tickets, first) = reading();
    let landed = state.land(
        first,
        1,
        Ok(PrFilesPage::of(full_page(0, "@@ -1 +1 @@\n+x"), 1)),
        None,
        || tickets.issue(),
    );
    let Landed::Next(second, 2) = landed else {
        panic!("a full page leads to the next: {landed:?}");
    };
    assert_eq!(state.len(), 100);
    assert!(!state.complete());
    assert_eq!(
        state.land(
            first,
            1,
            Ok(PrFilesPage::of(vec![edit("late")], 1)),
            None,
            || { tickets.issue() }
        ),
        Landed::Stale,
        "an answer already taken is not taken twice"
    );
    let landed = state.land(
        second,
        2,
        Ok(PrFilesPage::of(vec![edit("last")], 2)),
        None,
        || tickets.issue(),
    );
    assert_eq!(landed, Landed::Settled);
    assert_eq!(state.len(), 101);
    assert!(state.complete());
    assert_eq!(
        state.shown.as_ref().unwrap().path,
        "f0",
        "the first file is shown"
    );
}

#[test]
fn reading_pauses_at_the_budget_and_resumes_on_request() {
    let (mut state, mut tickets, first) = reading();
    let big = format!(
        "@@ -1 +1 @@\n+{}",
        "x".repeat(AUTO_READ_BYTES / PR_FILES_PER_PAGE)
    );
    let landed = state.land(
        first,
        1,
        Ok(PrFilesPage::of(full_page(0, &big), 1)),
        None,
        || tickets.issue(),
    );
    assert_eq!(
        landed,
        Landed::Settled,
        "the budget is spent: no page follows on its own"
    );
    let FilesView::Ready { more, .. } = files_view(Some(&state), Some(300), unmarked) else {
        panic!("ready");
    };
    assert_eq!(more, FilesMore::Paused);
    let ticket = tickets.issue();
    assert_eq!(state.resume(ticket), Some(2), "asked, it reads on");
    assert_eq!(state.resume(tickets.issue()), None, "not twice at once");
}

#[test]
fn a_failed_page_keeps_what_came_before_and_is_tried_again() {
    let (mut state, mut tickets, first) = reading();
    let Landed::Next(second, 2) = state.land(
        first,
        1,
        Ok(PrFilesPage::of(full_page(0, "@@ -1 +1 @@\n+x"), 1)),
        None,
        || tickets.issue(),
    ) else {
        panic!("next");
    };
    state.land(second, 2, Err("HTTP 502".into()), None, || tickets.issue());
    let FilesView::Ready { files, more, .. } = files_view(Some(&state), None, unmarked) else {
        panic!("what came before stays listed");
    };
    assert_eq!(files.len(), 100);
    assert_eq!(more, FilesMore::Failed("HTTP 502".into()));
    assert!(
        !state.wants_read(),
        "a listing is up: the tab does not start over"
    );
    assert_eq!(
        state.resume(tickets.issue()),
        Some(2),
        "the failed page is read again"
    );

    let (mut empty, mut tickets, first) = reading();
    empty.land(first, 1, Err("HTTP 502".into()), None, || tickets.issue());
    assert_eq!(
        files_view(Some(&empty), None, unmarked),
        FilesView::Failed("HTTP 502".into())
    );
    assert!(
        empty.wants_read(),
        "nothing listed: shown again, it reads again"
    );
}

#[test]
fn a_reread_keeps_the_listing_and_choice_until_its_first_page() {
    let (mut state, mut tickets, first) = reading();
    state.land(
        first,
        1,
        Ok(PrFilesPage::of(vec![edit("a"), edit("b")], 1)),
        None,
        || tickets.issue(),
    );
    state.show("b");
    let again = tickets.issue();
    state.reread(again);
    assert_eq!(state.len(), 2, "still listed while read again");
    assert!(!state.complete());
    state.land(
        again,
        1,
        Ok(PrFilesPage::of(vec![edit("b"), edit("c")], 1)),
        None,
        || tickets.issue(),
    );
    let paths: Vec<&str> = state.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["b", "c"], "replaced, not appended");
    assert_eq!(state.shown.as_ref().unwrap().path, "b", "the choice stays");
}

#[test]
fn the_diff_is_drawn_in_steps() {
    let (mut state, mut tickets, first) = reading();
    state.land(
        first,
        1,
        Ok(PrFilesPage::of(vec![edit("a")], 1)),
        None,
        || tickets.issue(),
    );
    assert_eq!(state.shown.as_ref().unwrap().limit, DIFF_ROWS_STEP);
    state.show_more_rows();
    assert_eq!(state.shown.as_ref().unwrap().limit, 2 * DIFF_ROWS_STEP);
    state.show("a");
    assert_eq!(
        state.shown.as_ref().unwrap().limit,
        2 * DIFF_ROWS_STEP,
        "choosing the shown file again keeps how much is drawn"
    );
}

#[test]
fn a_file_reads_as_its_kind_note_and_diff() {
    let (mut state, mut tickets, first) = reading();
    state.land(
        first,
        1,
        Ok(PrFilesPage::of(
            vec![
                file(
                    "new/path",
                    FileChange::Renamed {
                        from: "old/path".into(),
                    },
                    Some("@@ -1 +1 @@\n-a\n+b"),
                ),
                file("bin", FileChange::Removed, None),
            ],
            1,
        )),
        None,
        || tickets.issue(),
    );
    let FilesView::Ready {
        files,
        shown,
        total,
        ..
    } = files_view(Some(&state), Some(2), unmarked)
    else {
        panic!("ready");
    };
    assert_eq!(total, None, "every page in: the listing is the count");
    assert_eq!(files[0].kind, ChangeKind::Renamed);
    assert!(files[0].note.as_deref().unwrap().contains("old/path"));
    assert_eq!(files[1].kind, ChangeKind::Removed);
    assert!(files[0].shown && !files[1].shown);
    assert!(matches!(shown.map(|s| s.diff), Some(FileDiff::Rows(_))));

    let sized = |change, additions, patch: Option<&str>| PrFile {
        path: "x".into(),
        change,
        additions,
        deletions: 0,
        patch: patch.map(str::to_owned),
        blob: None,
    };
    let renamed = || FileChange::Renamed { from: "y".into() };
    // (label, file, diff)
    let cases = [
        (
            "lines but no patch: too large",
            sized(FileChange::Modified, 900, None),
            FileDiff::TooLarge,
        ),
        (
            "no lines and no patch: binary",
            sized(FileChange::Added, 0, None),
            FileDiff::NoLines,
        ),
        (
            "an empty patch: no line changed",
            sized(FileChange::Changed, 0, Some("")),
            FileDiff::NoLines,
        ),
        (
            "a patch with no hunk",
            sized(FileChange::Changed, 0, Some("Binary files differ")),
            FileDiff::NoLines,
        ),
        (
            "a rename without an edit, empty patch",
            sized(renamed(), 0, Some("")),
            FileDiff::MovedOnly,
        ),
        (
            "a rename without an edit, no patch",
            sized(renamed(), 0, None),
            FileDiff::MovedOnly,
        ),
        (
            "a rename with edits too large",
            sized(renamed(), 9, None),
            FileDiff::TooLarge,
        ),
    ];
    for (label, file, diff) in cases {
        assert_eq!(FileDiff::of(&file, Unchanged::Shown), diff, "{label}");
    }
    assert_eq!(files_view(None, None, unmarked), FilesView::Loading);
}

#[test]
fn a_later_page_keeps_the_shown_diff_and_how_much_is_drawn() {
    let (mut state, mut tickets, first) = reading();
    let Landed::Next(second, 2) = state.land(
        first,
        1,
        Ok(PrFilesPage::of(full_page(0, "@@ -1 +1 @@\n+x"), 1)),
        None,
        || tickets.issue(),
    ) else {
        panic!("next");
    };
    state.show_more_rows();
    let rows = state.shown.as_ref().unwrap().diff.clone();
    state.land(
        second,
        2,
        Ok(PrFilesPage::of(vec![edit("z")], 2)),
        None,
        || tickets.issue(),
    );
    let shown = state.shown.as_ref().unwrap();
    assert_eq!(
        shown.limit,
        2 * DIFF_ROWS_STEP,
        "show more survives a page landing"
    );
    assert_eq!(shown.diff, rows, "and the rows are not rebuilt");
}

#[test]
fn a_refresh_keeps_a_choice_beyond_its_first_page() {
    let (mut state, mut tickets, first) = reading();
    let Landed::Next(second, 2) = state.land(
        first,
        1,
        Ok(PrFilesPage::of(full_page(0, "@@ -1 +1 @@\n+x"), 1)),
        None,
        || tickets.issue(),
    ) else {
        panic!("next");
    };
    state.land(
        second,
        2,
        Ok(PrFilesPage::of(vec![edit("far")], 2)),
        None,
        || tickets.issue(),
    );
    state.show("far");
    state.show_more_rows();

    let again = tickets.issue();
    state.reread(again);
    let Landed::Next(again2, 2) = state.land(
        again,
        1,
        Ok(PrFilesPage::of(full_page(0, "@@ -1 +1 @@\n+x"), 1)),
        None,
        || tickets.issue(),
    ) else {
        panic!("next");
    };
    assert_eq!(
        state.chosen.as_deref(),
        Some("far"),
        "kept while its page is still to come"
    );
    assert_eq!(
        state.shown.as_ref().unwrap().path,
        "far",
        "its last rows stay up meanwhile"
    );
    state.land(
        again2,
        2,
        Ok(PrFilesPage::of(vec![edit("far")], 2)),
        None,
        || tickets.issue(),
    );
    let shown = state.shown.as_ref().unwrap();
    assert_eq!(shown.path, "far");
    assert_eq!(shown.limit, 2 * DIFF_ROWS_STEP);

    let last = tickets.issue();
    state.reread(last);
    state.land(
        last,
        1,
        Ok(PrFilesPage::of(vec![edit("only")], 1)),
        None,
        || tickets.issue(),
    );
    assert_eq!(
        state.shown.as_ref().unwrap().path,
        "only",
        "a choice gone once every page is in gives way to the first file"
    );
}

#[test]
fn reading_the_rest_reads_every_remaining_page() {
    let (mut state, mut tickets, first) = reading();
    let big = format!(
        "@@ -1 +1 @@\n+{}",
        "x".repeat(AUTO_READ_BYTES / PR_FILES_PER_PAGE)
    );
    state.land(
        first,
        1,
        Ok(PrFilesPage::of(full_page(0, &big), 1)),
        None,
        || tickets.issue(),
    );
    let ticket = tickets.issue();
    assert_eq!(state.resume(ticket), Some(2));
    let landed = state.land(
        ticket,
        2,
        Ok(PrFilesPage::of(full_page(100, &big), 2)),
        None,
        || tickets.issue(),
    );
    assert!(
        matches!(landed, Landed::Next(_, 3)),
        "past the budget once asked: {landed:?}"
    );
}

#[test]
fn a_refresh_mid_paging_drops_the_older_page_and_a_failed_refresh_keeps_the_listing() {
    let (mut state, mut tickets, first) = reading();
    let Landed::Next(second, 2) = state.land(
        first,
        1,
        Ok(PrFilesPage::of(full_page(0, "@@ -1 +1 @@\n+x"), 1)),
        None,
        || tickets.issue(),
    ) else {
        panic!("next");
    };
    let again = tickets.issue();
    state.reread(again);
    assert_eq!(
        state.land(
            second,
            2,
            Ok(PrFilesPage::of(vec![edit("old")], 2)),
            None,
            || { tickets.issue() }
        ),
        Landed::Stale,
        "the earlier read's page does not join the new read"
    );
    state.land(again, 1, Err("HTTP 502".into()), None, || tickets.issue());
    let FilesView::Ready { files, more, .. } = files_view(Some(&state), None, unmarked) else {
        panic!("the earlier listing stays");
    };
    assert_eq!(files.len(), 100);
    assert_eq!(more, FilesMore::RefreshFailed("HTTP 502".into()));
    let retry = tickets.issue();
    assert_eq!(
        state.resume(retry),
        Some(1),
        "Try again reads the first page again"
    );
    assert!(state.replacing, "still in place of the earlier listing");
}

fn landed(paths: &[&str], patch: &str) -> (FilesState, Tickets) {
    let (mut state, mut tickets, first) = reading();
    let files = paths
        .iter()
        .map(|path| file(path, FileChange::Modified, Some(patch)))
        .collect();
    state.land(first, 1, Ok(PrFilesPage::of(files, 1)), None, || {
        tickets.issue()
    });
    (state, tickets)
}

#[test]
fn the_previous_and_next_file_stop_at_either_end() {
    let (mut state, _) = landed(&["a", "b", "c"], "@@ -1 +1 @@\n-x\n+y");
    let shown = |state: &FilesState| state.shown.as_ref().unwrap().path.clone();
    state.show_adjacent(-1);
    assert_eq!(shown(&state), "a", "no file before the first");
    state.show_adjacent(1);
    state.show_adjacent(1);
    assert_eq!(shown(&state), "c");
    state.show_adjacent(1);
    assert_eq!(shown(&state), "c", "none after the last");
}

#[test]
fn folding_unchanged_lines_rebuilds_the_diff_keeping_how_much_is_drawn() {
    let (mut state, _) = landed(&["a"], "@@ -1,3 +1,3 @@\n keep\n-x\n+y\n tail");
    let rows = |state: &FilesState| match &state.shown.as_ref().unwrap().diff {
        FileDiff::Rows(rows) => rows.rows().len(),
        other => panic!("{other:?}"),
    };
    assert_eq!(rows(&state), 5, "header, two unchanged, removed, added");
    state.show_more_rows();
    state.set_unchanged(Unchanged::Folded);
    assert_eq!(rows(&state), 3, "the unchanged lines fold away");
    assert_eq!(state.shown.as_ref().unwrap().limit, 2 * DIFF_ROWS_STEP);
}

#[test]
fn lines_are_pinned_to_the_commit_their_rows_were_read_at() {
    let (mut state, mut tickets, first) = reading();
    let page = || {
        PrFilesPage::of(
            vec![file("a", FileChange::Modified, Some("@@ -1 +1 @@\n-x\n+y"))],
            1,
        )
    };
    // The pull request was being read too: the files' commit is unknown.
    state.land(first, 1, Ok(page()), None, || tickets.issue());
    state.select_line(Side::New, 1, false);
    assert!(state.selection().is_none(), "no commit to pin it to");
    assert!(
        !state.head_read("abc1234"),
        "an unknown listing takes the head"
    );
    state.select_line(Side::New, 1, false);
    state.select_line(Side::New, 4, true);
    let held = state.selection().unwrap();
    assert_eq!(
        (
            held.path.as_str(),
            held.start,
            held.end,
            held.revision.as_str()
        ),
        ("a", 1, 4, "abc1234")
    );
    assert!(
        !state.head_read("abc1234"),
        "the same head keeps everything"
    );
    assert!(state.selection().is_some());
    assert!(
        state.head_read("def5678"),
        "a moved head asks for the files again"
    );
    assert!(
        state.selection().is_some(),
        "until rows at the new head replace them"
    );

    // The refresh's first page lands at the new head: its rows replace
    // the old, and the lines chosen in the old go with them.
    let again = tickets.issue();
    state.reread(again);
    assert!(
        !state.head_read("def5678"),
        "a first page still out takes the head itself"
    );
    state.land(again, 1, Ok(page()), Some("def5678"), || tickets.issue());
    assert!(state.selection().is_none());
}

#[test]
fn a_refresh_keeps_earlier_rows_under_their_own_commit() {
    let (mut state, mut tickets, first) = reading();
    let files = |paths: &[&str]| {
        paths
            .iter()
            .map(|path| file(path, FileChange::Modified, Some("@@ -1 +1 @@\n-x\n+y")))
            .collect::<Vec<_>>()
    };
    let full = |prefix: &str| {
        (0..PR_FILES_PER_PAGE)
            .map(|n| {
                file(
                    &format!("{prefix}{n}"),
                    FileChange::Modified,
                    Some("@@ -1 +1 @@\n-x\n+y"),
                )
            })
            .collect::<Vec<_>>()
    };
    let Landed::Next(second, 2) = state.land(
        first,
        1,
        Ok(PrFilesPage::of(full("p"), 1)),
        Some("old"),
        || tickets.issue(),
    ) else {
        panic!("a full page leads on");
    };
    state.land(
        second,
        2,
        Ok(PrFilesPage::of(files(&["far"]), 2)),
        Some("old"),
        || tickets.issue(),
    );
    state.show("far");
    let again = tickets.issue();
    state.reread(again);
    // The new first page does not list the chosen file: its rows stay up,
    // still pinned to the commit they were read at.
    state.land(
        again,
        1,
        Ok(PrFilesPage::of(full("q"), 1)),
        Some("new"),
        || tickets.issue(),
    );
    state.select_line(Side::New, 1, false);
    assert_eq!(state.selection().unwrap().revision, "old");
}

#[test]
fn a_reopened_detail_comes_back_to_its_file_and_scrolls_only_to_unchanged_content() {
    let place = |blob: &str| PlaceFile {
        path: "far".into(),
        blob: Some(blob.into()),
    };
    // (label, blob come back to, the page's patch for "far", offset restored)
    let cases = [
        ("the same content", "far@P", "P", Some((0.0, -300.0))),
        ("changed content", "far@P", "Q", None),
    ];
    for (label, blob, patch, offset) in cases {
        let (mut state, mut tickets, first) = reading();
        state.restore_to(&place(blob), (0.0, -300.0));
        let Landed::Next(second, 2) = state.land(
            first,
            1,
            Ok(PrFilesPage::of(full_page(0, "@@ -1 +1 @@\n-x\n+y"), 1)),
            None,
            || tickets.issue(),
        ) else {
            panic!("{label}: a full page leads on");
        };
        assert_eq!(
            state.take_restored(),
            None,
            "{label}: not listed yet, still waiting"
        );
        let far = file("far", FileChange::Modified, Some(patch));
        state.land(second, 2, Ok(PrFilesPage::of(vec![far], 2)), None, || {
            tickets.issue()
        });
        let restored = state.take_restored().expect(label);
        assert_eq!(restored.index, PR_FILES_PER_PAGE, "{label}");
        assert_eq!(restored.offset, offset, "{label}");
        assert_eq!(state.take_restored(), None, "{label}: given once");
    }
}

#[test]
fn coming_back_to_a_file_no_longer_listed_shows_the_first_and_gives_up() {
    let (mut state, mut tickets, first) = reading();
    state.restore_to(
        &PlaceFile {
            path: "gone".into(),
            blob: Some("b".into()),
        },
        (0.0, -10.0),
    );
    state.land(
        first,
        1,
        Ok(PrFilesPage::of(vec![edit("a")], 1)),
        None,
        || tickets.issue(),
    );
    assert_eq!(state.take_restored(), None);
    assert_eq!(state.place_file().map(|file| file.path), Some("a".into()));
    assert!(state.restore.is_none(), "reading ended without it");
}

#[test]
fn a_file_read_whole_lands_only_on_the_listing_it_was_read_for() {
    let big = || PrFile {
        additions: 9_000,
        ..file("big", FileChange::Modified, None)
    };
    let (mut state, mut tickets, first) = reading();
    state.land(
        first,
        1,
        Ok(PrFilesPage::of(vec![big()], 1)),
        Some("h1"),
        || tickets.issue(),
    );
    let read = tickets.issue();
    assert_eq!(
        state
            .begin_recover(read)
            .map(|start| (start.path, start.head)),
        Some(("big".into(), "h1".into()))
    );
    // A refresh lists the files again at a new head before the read lands.
    let again = tickets.issue();
    state.reread(again);
    state.land(
        again,
        1,
        Ok(PrFilesPage::of(vec![big()], 1)),
        Some("h2"),
        || tickets.issue(),
    );
    assert!(!state.land_recovered(read, Ok(recovered())));
    assert_eq!(
        state.shown.as_ref().unwrap().diff,
        FileDiff::TooLarge,
        "not on code read at h1"
    );

    let read = tickets.issue();
    state.begin_recover(read);
    assert!(
        state.begin_recover(tickets.issue()).is_none(),
        "one at a time"
    );
    assert!(state.land_recovered(read, Ok(recovered())));
    assert!(matches!(
        state.shown.as_ref().unwrap().diff,
        FileDiff::Rows(_)
    ));
    assert_eq!(
        state.merge_base,
        Some(("h2".into(), "base".into())),
        "kept for the listing's other files"
    );
}

fn recovered() -> crate::workspace::pages::pull_requests::detail::recover::Recovered {
    crate::workspace::pages::pull_requests::detail::recover::Recovered {
        patch: "@@ -1 +1 @@\n-a\n+b".into(),
        merge_base: "base".into(),
    }
}
