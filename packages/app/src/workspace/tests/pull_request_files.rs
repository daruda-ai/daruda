//! The detail's Files tab in a window: read when first shown, read again by
//! a refresh without taking an older answer, and showing the chosen diff.

use daruda_github::fake::GhOp;
use daruda_github::{FileChange, PrDetail, PrFile, PrFiles};
use gpui::TestAppContext;

use super::pull_request_detail::{mergeable, opened, repo};
use crate::workspace::pages::pull_requests::detail::diff_layout::Side;
use crate::workspace::pages::pull_requests::detail::files::{FileDiff, FilesView, RecoveryView};
use crate::workspace::pages::pull_requests::detail::progress::Viewed;
use crate::workspace::pages::pull_requests::detail::recover::RecoverFailure;
use crate::workspace::pages::pull_requests::detail::{PrDetailId, PrTab};

fn file(path: &str, patch: Option<&str>) -> PrFile {
    PrFile {
        path: path.into(),
        change: FileChange::Modified,
        additions: 1,
        deletions: 1,
        patch: patch.map(str::to_owned),
        // Stands for the content: another patch is another blob.
        blob: Some(format!("{path}@{}", patch.unwrap_or_default())),
    }
}

const PATCH: &str = "@@ -1,2 +1,2 @@\n context\n-old\n+new";

fn files_view(f: &super::GitFixture, cx: &mut TestAppContext) -> FilesView {
    f.ws.update(cx, |ws, cx| {
        ws.stage_pr_page(cx);
        let snap = ws.pages.pull_requests.detail_snapshot.as_ref().unwrap();
        match &snap.body {
            crate::workspace::pages::pull_requests::detail::view::DetailBody::Ready(ready) => {
                ready.files.clone().expect("the Files tab is shown")
            }
            other => panic!("detail not ready: {other:?}"),
        }
    })
}

fn select(f: &super::GitFixture, id: PrDetailId, cx: &mut TestAppContext) {
    f.ws.update(cx, |ws, cx| ws.select_pr_detail_tab(id, PrTab::Files, cx));
    cx.run_until_parked();
}

#[gpui::test]
fn the_files_are_read_once_the_tab_is_shown_and_show_the_first_diff(cx: &mut TestAppContext) {
    let (f, gh, id) = opened(cx, mergeable(7, "feat"));
    gh.set_pr_files(
        &repo(),
        7,
        PrFiles {
            files: vec![
                file("src/a.rs", Some(PATCH)),
                PrFile {
                    additions: 0,
                    deletions: 0,
                    ..file("logo.png", None)
                },
            ],
        },
    );
    assert_eq!(
        gh.count(GhOp::PrFiles),
        0,
        "not read before the tab is shown"
    );
    select(&f, id, cx);
    select(&f, id, cx);
    assert_eq!(gh.count(GhOp::PrFiles), 1, "read once, however often shown");

    let FilesView::Ready { files, shown, .. } = files_view(&f, cx) else {
        panic!("files read");
    };
    assert_eq!(files.len(), 2);
    assert!(files[0].shown, "the first file is shown at once");
    let Some(shown) = shown else {
        panic!("a diff");
    };
    let FileDiff::Rows(rows) = shown.diff else {
        panic!("rows");
    };
    assert_eq!(shown.path, "src/a.rs");
    assert_eq!(rows.rows().len(), 4, "the hunk header and three lines");

    f.ws.update(cx, |ws, cx| ws.show_pr_file(id, "logo.png", cx));
    let FilesView::Ready { shown, .. } = files_view(&f, cx) else {
        panic!("files read");
    };
    assert_eq!(
        shown.map(|shown| (shown.path, shown.diff)),
        Some(("logo.png".to_owned(), FileDiff::NoLines)),
        "a binary file says no line changed"
    );
}

#[gpui::test]
fn a_files_read_outdated_by_a_refresh_does_not_land(cx: &mut TestAppContext) {
    let (f, gh, id) = opened(cx, mergeable(7, "feat"));
    let one = |path: &str| PrFiles {
        files: vec![file(path, Some(PATCH))],
    };
    gh.set_pr_files(&repo(), 7, one("new.rs"));
    f.ws.update(cx, |ws, cx| {
        ws.select_pr_detail_tab(id, PrTab::Files, cx);
        let detail = ws.pages.pull_requests.detail.as_ref().unwrap();
        let (first, page) = detail.files().unwrap().waiting().unwrap();
        ws.refresh_pr_detail(id, cx);
        let old = daruda_github::PrFilesPage::of(one("old.rs").files, page);
        ws.land_pr_files(id, first, page, Ok(old), cx);
    });
    assert_eq!(
        files_view(&f, cx),
        FilesView::Loading,
        "the old answer is dropped"
    );
    cx.run_until_parked();
    let FilesView::Ready { shown, .. } = files_view(&f, cx) else {
        panic!("files read");
    };
    assert_eq!(shown.map(|shown| shown.path).as_deref(), Some("new.rs"));
    assert_eq!(gh.count(GhOp::PrFiles), 2, "the refresh read them again");
}

fn two() -> PrFiles {
    PrFiles {
        files: vec![file("a.rs", Some(PATCH)), file("b.rs", Some(PATCH))],
    }
}

fn shown_path(f: &super::GitFixture, cx: &mut TestAppContext) -> Option<String> {
    match files_view(f, cx) {
        FilesView::Ready { shown, .. } => shown.map(|shown| shown.path),
        other => panic!("files not listed: {other:?}"),
    }
}

#[gpui::test]
fn a_refresh_keeps_the_listing_and_the_chosen_file(cx: &mut TestAppContext) {
    let (f, gh, id) = opened(cx, mergeable(7, "feat"));
    gh.set_pr_files(&repo(), 7, two());
    select(&f, id, cx);
    f.ws.update(cx, |ws, cx| ws.show_pr_file(id, "b.rs", cx));
    f.ws.update(cx, |ws, cx| ws.refresh_pr_detail(id, cx));
    assert_eq!(
        shown_path(&f, cx).as_deref(),
        Some("b.rs"),
        "the listing stays up while it is read again"
    );
    cx.run_until_parked();
    assert_eq!(gh.count(GhOp::PrFiles), 2);
    assert_eq!(
        shown_path(&f, cx).as_deref(),
        Some("b.rs"),
        "and the chosen file with it"
    );

    gh.fail(
        GhOp::PrFiles,
        daruda_github::fake::GhFailure::Exit {
            code: Some(1),
            stderr: "HTTP 502".into(),
        },
    );
    f.ws.update(cx, |ws, cx| ws.refresh_pr_detail(id, cx));
    cx.run_until_parked();
    assert_eq!(
        shown_path(&f, cx).as_deref(),
        Some("b.rs"),
        "a refresh that fails leaves the last listing up"
    );
}

#[gpui::test]
fn a_refresh_on_another_tab_reads_the_files_when_shown_again(cx: &mut TestAppContext) {
    let (f, gh, id) = opened(cx, mergeable(7, "feat"));
    gh.set_pr_files(&repo(), 7, two());
    select(&f, id, cx);
    f.ws.update(cx, |ws, cx| {
        ws.select_pr_detail_tab(id, PrTab::Checks, cx);
        ws.refresh_pr_detail(id, cx);
    });
    cx.run_until_parked();
    assert_eq!(gh.count(GhOp::PrFiles), 1, "not read while hidden");
    select(&f, id, cx);
    assert_eq!(gh.count(GhOp::PrFiles), 2, "read when shown again");
}

#[gpui::test]
fn a_failed_read_is_tried_again_when_the_tab_is_shown_again(cx: &mut TestAppContext) {
    let (f, gh, id) = opened(cx, mergeable(7, "feat"));
    gh.fail(
        GhOp::PrFiles,
        daruda_github::fake::GhFailure::Exit {
            code: Some(1),
            stderr: "HTTP 502".into(),
        },
    );
    select(&f, id, cx);
    assert!(matches!(files_view(&f, cx), FilesView::Failed(_)));
    gh.clear_failure(GhOp::PrFiles);
    gh.set_pr_files(&repo(), 7, two());
    f.ws.update(cx, |ws, cx| ws.select_pr_detail_tab(id, PrTab::Checks, cx));
    select(&f, id, cx);
    assert_eq!(gh.count(GhOp::PrFiles), 2);
    assert_eq!(shown_path(&f, cx).as_deref(), Some("a.rs"));
}

#[gpui::test]
fn a_large_pull_request_is_listed_page_by_page_with_its_count(cx: &mut TestAppContext) {
    let mut detail = mergeable(7, "feat");
    detail.changed_files = Some(250);
    let (f, gh, id) = opened(cx, detail);
    gh.set_pr_files(
        &repo(),
        7,
        PrFiles {
            files: (0..250)
                .map(|n| file(&format!("f{n}.rs"), Some(PATCH)))
                .collect(),
        },
    );
    select(&f, id, cx);
    assert_eq!(gh.count(GhOp::PrFiles), 3, "three pages, read in turn");
    let FilesView::Ready {
        files, total, more, ..
    } = files_view(&f, cx)
    else {
        panic!("listed");
    };
    assert_eq!(files.len(), 250);
    assert_eq!(total, None, "every page in: the listing is the count");
    assert_eq!(
        more,
        crate::workspace::pages::pull_requests::detail::files::FilesMore::None
    );
}

#[gpui::test]
fn a_long_line_scrolls_sideways_and_a_long_diff_is_drawn_in_steps(cx: &mut TestAppContext) {
    use gpui::{Modifiers, VisualTestContext, px, size};
    let (f, gh, id) = opened(cx, mergeable(7, "feat"));
    let long = format!("@@ -1 +1 @@\n+{}", "x".repeat(400));
    let rows: String = (0..2_500).map(|n| format!("\n+line {n}")).collect();
    gh.set_pr_files(
        &repo(),
        7,
        PrFiles {
            files: vec![
                file("wide.rs", Some(&long)),
                file("tall.rs", Some(&format!("@@ -0,0 +1,2500 @@{rows}"))),
            ],
        },
    );
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.simulate_resize(size(px(1000.), px(800.)));
    f.ws.update(&mut vcx, |ws, cx| {
        ws.select_pr_detail_tab(id, PrTab::Files, cx)
    });
    vcx.run_until_parked();
    let view = vcx.debug_bounds("pr-file-diff-scroll").unwrap();
    let content = vcx.debug_bounds("pr-file-diff-rows").unwrap();
    assert!(
        content.size.width > view.size.width,
        "the line keeps its length inside a sideways scroll: {content:?} in {view:?}"
    );

    f.ws.update(&mut vcx, |ws, cx| ws.show_pr_file(id, "tall.rs", cx));
    vcx.run_until_parked();
    let FilesView::Ready { shown, .. } = files_view(&f, &mut vcx) else {
        panic!("listed");
    };
    assert_eq!(shown.as_ref().unwrap().limit, 1_000, "a first step of rows");
    let scroll = f.ws.read_with(&vcx, |ws, _| {
        ws.pages
            .pull_requests
            .detail
            .as_ref()
            .unwrap()
            .scroll()
            .clone()
    });
    scroll.set_offset(gpui::point(px(0.), -scroll.max_offset().y));
    f.ws.update(&mut vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let more = vcx
        .debug_bounds("pr-file-diff-more")
        .expect("the rest is offered");
    vcx.simulate_click(more.center(), Modifiers::default());
    vcx.run_until_parked();
    let FilesView::Ready { shown, .. } = files_view(&f, &mut vcx) else {
        panic!("listed");
    };
    assert_eq!(shown.unwrap().limit, 2_000);
}

#[gpui::test]
fn lines_are_selected_from_the_gutter_and_copied_as_a_reference(cx: &mut TestAppContext) {
    use gpui::{Modifiers, VisualTestContext, px, size};
    let mut detail = mergeable(7, "feat");
    detail.head_sha = "4f9c2e1abcdef".into();
    let (f, gh, id) = opened(cx, detail);
    gh.set_pr_files(
        &repo(),
        7,
        PrFiles {
            files: vec![file(
                "src/a.rs",
                Some("@@ -1,3 +1,4 @@\n one\n-two\n+two!\n+three\n four"),
            )],
        },
    );
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.simulate_resize(size(px(1400.), px(900.)));
    f.ws.update(&mut vcx, |ws, cx| {
        ws.select_pr_detail_tab(id, PrTab::Files, cx)
    });
    vcx.run_until_parked();

    // Row 3 is "+two!" (new line 2); row 4 is "+three" (new line 3).
    let new_number = |vcx: &mut VisualTestContext, row: usize| {
        let bounds = vcx
            .debug_bounds(Box::leak(format!("pr-diff-row-{row}").into_boxed_str()))
            .unwrap();
        gpui::point(
            bounds.left() + px(1.5 * crate::ui::theme::PR_DIFF_LINE_NO_W),
            bounds.center().y,
        )
    };
    let at = new_number(&mut vcx, 3);
    vcx.simulate_click(at, Modifiers::default());
    let at = new_number(&mut vcx, 4);
    vcx.simulate_click(at, Modifiers::shift());
    vcx.run_until_parked();
    assert!(
        vcx.debug_bounds("pr-lines-selected").is_some(),
        "the selection is shown"
    );
    let copy = vcx.debug_bounds("pr-lines-copy").unwrap();
    vcx.simulate_click(copy.center(), Modifiers::default());
    let copied = vcx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()));
    assert_eq!(
        copied.as_deref(),
        Some("src/a.rs:2-3 (new) @ 4f9c2e1\ntwo!\nthree")
    );

    let clear = vcx.debug_bounds("pr-lines-clear").unwrap();
    vcx.simulate_click(clear.center(), Modifiers::default());
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("pr-lines-selected").is_none());
}

#[gpui::test]
fn the_diff_lays_out_side_by_side_and_folds_and_files_step_in_order(cx: &mut TestAppContext) {
    use gpui::{Modifiers, VisualTestContext, px, size};
    let (f, gh, id) = opened(cx, mergeable(7, "feat"));
    let patch = "@@ -1,3 +1,3 @@\n keep\n-old\n+new\n tail";
    gh.set_pr_files(
        &repo(),
        7,
        PrFiles {
            files: vec![file("a.rs", Some(patch)), file("b.rs", Some(patch))],
        },
    );
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.simulate_resize(size(px(1600.), px(900.)));
    f.ws.update(&mut vcx, |ws, cx| {
        ws.select_pr_detail_tab(id, PrTab::Files, cx)
    });
    vcx.run_until_parked();
    assert!(
        vcx.debug_bounds("pr-diff-row-0").is_some(),
        "unified at first"
    );

    let split = vcx.debug_bounds("pr-diff-mode-1").unwrap();
    vcx.simulate_click(split.center(), Modifiers::default());
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("pr-diff-row-0").is_none());
    // Header, keep, old|new, tail.
    assert!(vcx.debug_bounds("pr-split-row-3").is_some());
    assert!(
        vcx.debug_bounds("pr-split-row-4").is_none(),
        "old and new share a row"
    );

    let fold = vcx.debug_bounds("pr-file-unchanged").unwrap();
    vcx.simulate_click(fold.center(), Modifiers::default());
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("pr-split-row-1").is_some());
    assert!(
        vcx.debug_bounds("pr-split-row-2").is_none(),
        "header and the changed pair only"
    );

    let next = vcx.debug_bounds("pr-file-next").unwrap();
    vcx.simulate_click(next.center(), Modifiers::default());
    vcx.run_until_parked();
    let FilesView::Ready { shown, .. } = files_view(&f, &mut vcx) else {
        panic!("listed");
    };
    let shown = shown.unwrap();
    assert_eq!(shown.path, "b.rs");
    assert_eq!(shown.position, Some((1, 2)));
}

#[gpui::test]
fn a_long_listing_draws_only_the_rows_in_view(cx: &mut TestAppContext) {
    use gpui::{VisualTestContext, px, size};
    let (f, gh, id) = opened(cx, mergeable(7, "feat"));
    gh.set_pr_files(
        &repo(),
        7,
        PrFiles {
            files: (0..300)
                .map(|n| file(&format!("f{n}.rs"), Some(PATCH)))
                .collect(),
        },
    );
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.simulate_resize(size(px(1400.), px(900.)));
    f.ws.update(&mut vcx, |ws, cx| {
        ws.select_pr_detail_tab(id, PrTab::Files, cx)
    });
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("pr-file-0").is_some());
    assert!(
        vcx.debug_bounds("pr-file-299").is_none(),
        "a row out of view is not drawn"
    );
    let list = vcx.debug_bounds("pr-files").unwrap();
    assert!(list.size.height <= px(crate::ui::theme::PR_FILES_LIST_MAX_H + 2.0));
}

/// Select a line at head `aaaa111`, refresh to head `bbbb222`, and land the
/// two reads in the order given.
fn refreshed_to_a_new_head(cx: &mut TestAppContext, files_first: bool) -> Option<String> {
    let mut detail = mergeable(7, "feat");
    detail.head_sha = "aaaa111".into();
    let (f, gh, id) = opened(cx, detail.clone());
    gh.set_pr_files(
        &repo(),
        7,
        PrFiles {
            files: vec![file("a.rs", Some(PATCH))],
        },
    );
    select(&f, id, cx);
    f.ws.update(cx, |ws, cx| {
        ws.select_pr_line(id, Side::New, 1, false, cx);
        ws.refresh_pr_detail(id, cx);
        let state = ws.pages.pull_requests.detail.as_ref().unwrap();
        let read = state.pr_waiting().unwrap();
        let (files, page) = state.files().unwrap().waiting().unwrap();
        let moved = PrDetail {
            head_sha: "bbbb222".into(),
            ..detail
        };
        let page_read = || daruda_github::PrFilesPage::of(vec![file("a.rs", Some(PATCH))], page);
        if files_first {
            ws.land_pr_files(id, files, page, Ok(page_read()), cx);
            ws.land_pr_detail(id, read, Ok(moved), cx);
        } else {
            ws.land_pr_detail(id, read, Ok(moved), cx);
            ws.land_pr_files(id, files, page, Ok(page_read()), cx);
        }
    });
    cx.run_until_parked();
    let selected = |f: &super::GitFixture, cx: &mut TestAppContext| {
        f.ws.read_with(cx, |ws, _| {
            let state = ws.pages.pull_requests.detail.as_ref().unwrap();
            state
                .files()
                .unwrap()
                .selection()
                .map(|held| held.revision.clone())
        })
    };
    assert_eq!(
        selected(&f, cx),
        None,
        "lines chosen at the old head are dropped"
    );
    f.ws.update(cx, |ws, cx| ws.select_pr_line(id, Side::New, 1, false, cx));
    selected(&f, cx)
}

#[gpui::test]
fn files_landing_before_the_new_head_take_it_when_it_lands(cx: &mut TestAppContext) {
    assert_eq!(
        refreshed_to_a_new_head(cx, true).as_deref(),
        Some("bbbb222")
    );
}

#[gpui::test]
fn files_landing_after_the_new_head_are_read_at_it(cx: &mut TestAppContext) {
    assert_eq!(
        refreshed_to_a_new_head(cx, false).as_deref(),
        Some("bbbb222")
    );
}

#[gpui::test]
fn stepping_through_files_keeps_the_shown_one_in_the_listing_view(cx: &mut TestAppContext) {
    use gpui::{VisualTestContext, px, size};
    let (f, gh, id) = opened(cx, mergeable(7, "feat"));
    gh.set_pr_files(
        &repo(),
        7,
        PrFiles {
            files: (0..40)
                .map(|n| file(&format!("f{n}.rs"), Some(PATCH)))
                .collect(),
        },
    );
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.simulate_resize(size(px(1400.), px(900.)));
    f.ws.update(&mut vcx, |ws, cx| {
        ws.select_pr_detail_tab(id, PrTab::Files, cx)
    });
    vcx.run_until_parked();
    assert!(
        vcx.debug_bounds("pr-file-30").is_none(),
        "out of view at first"
    );
    for _ in 0..30 {
        f.ws.update(&mut vcx, |ws, cx| ws.show_adjacent_pr_file(id, 1, cx));
    }
    vcx.run_until_parked();
    assert!(
        vcx.debug_bounds("pr-file-30").is_some(),
        "the shown file is brought into view"
    );
}

fn ready_view(
    f: &super::GitFixture,
    cx: &mut TestAppContext,
) -> (Vec<Viewed>, usize, Option<String>) {
    let FilesView::Ready {
        files,
        viewed,
        shown,
        ..
    } = files_view(f, cx)
    else {
        panic!("listed");
    };
    (
        files.iter().map(|row| row.viewed).collect(),
        viewed,
        shown.map(|shown| shown.path),
    )
}

#[gpui::test]
fn a_viewed_mark_lasts_until_its_file_changes(cx: &mut TestAppContext) {
    use gpui::{Modifiers, VisualTestContext, px, size};
    let (f, gh, id) = opened(cx, mergeable(7, "feat"));
    gh.set_pr_files(&repo(), 7, two());
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.simulate_resize(size(px(1400.), px(900.)));
    f.ws.update(&mut vcx, |ws, cx| {
        ws.select_pr_detail_tab(id, PrTab::Files, cx)
    });
    vcx.run_until_parked();
    for mark in ["pr-file-viewed-0", "pr-file-viewed-1"] {
        let at = vcx.debug_bounds(mark).unwrap().center();
        vcx.simulate_click(at, Modifiers::default());
    }
    vcx.run_until_parked();
    assert_eq!(ready_view(&f, &mut vcx).0, [Viewed::Yes, Viewed::Yes]);
    assert_eq!(
        ready_view(&f, &mut vcx).2.as_deref(),
        Some("a.rs"),
        "marking does not show a file"
    );

    // a.rs changes; b.rs does not.
    gh.set_pr_files(
        &repo(),
        7,
        PrFiles {
            files: vec![
                file("a.rs", Some("@@ -1 +1 @@\n-x\n+z")),
                file("b.rs", Some(PATCH)),
            ],
        },
    );
    f.ws.update(&mut vcx, |ws, cx| ws.refresh_pr_detail(id, cx));
    vcx.run_until_parked();
    let (marks, viewed, _) = ready_view(&f, &mut vcx);
    assert_eq!(
        marks,
        [Viewed::Changed, Viewed::Yes],
        "only the changed file's mark lapses"
    );
    assert_eq!(viewed, 1);

    let shown = vcx.debug_bounds("pr-file-viewed-shown").unwrap().center();
    vcx.simulate_click(shown, Modifiers::default());
    vcx.run_until_parked();
    assert_eq!(
        ready_view(&f, &mut vcx).0,
        [Viewed::Yes, Viewed::Yes],
        "marked again from the diff"
    );
}

/// Open #7 on b.rs, a long file, scrolled 600px down, and go back to the
/// list. Returns the offset it was left at.
fn leave_on_a_scrolled_file(
    f: &super::GitFixture,
    gh: &daruda_github::fake::FakeGh,
    id: PrDetailId,
    vcx: &mut gpui::VisualTestContext,
) -> gpui::Point<gpui::Pixels> {
    use gpui::{point, px, size};
    let long: String = std::iter::once("@@ -1,200 +1,200 @@".to_owned())
        .chain((0..200).map(|n| format!(" line {n}")))
        .collect::<Vec<_>>()
        .join("\n");
    gh.set_pr_files(
        &repo(),
        7,
        PrFiles {
            files: vec![file("a.rs", Some(PATCH)), file("b.rs", Some(&long))],
        },
    );
    vcx.simulate_resize(size(px(1400.), px(900.)));
    f.ws.update(vcx, |ws, cx| ws.select_pr_detail_tab(id, PrTab::Files, cx));
    vcx.run_until_parked();
    f.ws.update(vcx, |ws, cx| ws.show_pr_file(id, "b.rs", cx));
    vcx.run_until_parked();
    let offset = point(px(0.), px(-600.));
    f.ws.update(vcx, |ws, _| {
        ws.pages
            .pull_requests
            .detail
            .as_ref()
            .unwrap()
            .scroll()
            .set_offset(offset);
    });
    vcx.update(|window, cx| f.ws.update(cx, |ws, cx| ws.back_to_pr_list(window, cx)));
    vcx.run_until_parked();
    offset
}

fn detail_offset(
    f: &super::GitFixture,
    vcx: &mut gpui::VisualTestContext,
) -> gpui::Point<gpui::Pixels> {
    f.ws.read_with(vcx, |ws, _| {
        ws.pages
            .pull_requests
            .detail
            .as_ref()
            .unwrap()
            .scroll()
            .offset()
    })
}

#[gpui::test]
fn reopening_a_pull_request_comes_back_to_its_file_and_place(cx: &mut TestAppContext) {
    use super::pull_request_detail::{id as shown_id, item, open};
    let (f, gh, id) = opened(cx, mergeable(7, "feat"));
    let mut vcx = gpui::VisualTestContext::from_window(f.window.into(), cx);
    let offset = leave_on_a_scrolled_file(&f, &gh, id, &mut vcx);

    // Left again before it came back: the place still stands.
    open(&f, item(7, "feat"), &mut vcx);
    vcx.update(|window, cx| f.ws.update(cx, |ws, cx| ws.back_to_pr_list(window, cx)));
    vcx.run_until_parked();

    open(&f, item(7, "feat"), &mut vcx);
    vcx.run_until_parked();
    assert_ne!(shown_id(&f, &mut vcx), id, "a new detail");
    let (_, _, shown) = ready_view(&f, &mut vcx);
    assert_eq!(
        shown.as_deref(),
        Some("b.rs"),
        "on the Files tab, at the file left"
    );
    assert!(vcx.debug_bounds("pr-file-1").is_some(), "drawn");
    assert_eq!(
        detail_offset(&f, &mut vcx),
        offset,
        "scrolled back to where it was"
    );
}

#[gpui::test]
fn moving_on_before_a_reopened_detail_comes_back_keeps_it_where_it_is(cx: &mut TestAppContext) {
    use super::pull_request_detail::{id as shown_id, item, open};
    let (f, gh, id) = opened(cx, mergeable(7, "feat"));
    let mut vcx = gpui::VisualTestContext::from_window(f.window.into(), cx);
    leave_on_a_scrolled_file(&f, &gh, id, &mut vcx);

    open(&f, item(7, "feat"), &mut vcx);
    let again = shown_id(&f, &mut vcx);
    let coming_back = |f: &super::GitFixture, vcx: &mut gpui::VisualTestContext| {
        f.ws.read_with(vcx, |ws, _| {
            let detail = ws.pages.pull_requests.detail.as_ref().unwrap();
            detail.files().is_some_and(|files| files.coming_back())
        })
    };
    assert!(
        coming_back(&f, &mut vcx),
        "reopened on its file, not read yet"
    );
    f.ws.update(&mut vcx, |ws, cx| {
        ws.select_pr_detail_tab(again, PrTab::Checks, cx)
    });
    assert!(!coming_back(&f, &mut vcx), "moving on lets the place go");
    vcx.run_until_parked();
    assert_eq!(
        detail_offset(&f, &mut vcx),
        gpui::Point::default(),
        "the Checks tab is not scrolled to the file's place"
    );
}

#[gpui::test]
fn a_file_sent_without_a_patch_is_read_whole_on_request(cx: &mut TestAppContext) {
    use daruda_github::FileText;
    use gpui::{Modifiers, VisualTestContext, px, size};
    let mut detail = mergeable(7, "feat");
    detail.head_sha = "4f9c2e1".into();
    let base = format!("merge-base:{}...4f9c2e1", detail.base_branch);
    let (f, gh, id) = opened(cx, detail);
    gh.set_pr_files(
        &repo(),
        7,
        PrFiles {
            files: vec![
                PrFile {
                    additions: 5000,
                    ..file("big.rs", None)
                },
                PrFile {
                    additions: 9,
                    ..file("logo.bin", None)
                },
            ],
        },
    );
    gh.set_file_at(
        &repo(),
        "big.rs",
        &base,
        FileText::Text("keep\nold\n".into()),
    );
    gh.set_file_at(
        &repo(),
        "big.rs",
        "4f9c2e1",
        FileText::Text("keep\nnew\n".into()),
    );
    gh.set_file_at(&repo(), "logo.bin", &base, FileText::Binary);
    gh.set_file_at(&repo(), "logo.bin", "4f9c2e1", FileText::Binary);
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.simulate_resize(size(px(1400.), px(900.)));
    f.ws.update(&mut vcx, |ws, cx| {
        ws.select_pr_detail_tab(id, PrTab::Files, cx)
    });
    vcx.run_until_parked();
    assert!(
        vcx.debug_bounds("pr-diff-row-0").is_none(),
        "no patch, no rows"
    );
    let read = vcx.debug_bounds("pr-file-recover").unwrap().center();
    vcx.simulate_click(read, Modifiers::default());
    vcx.run_until_parked();
    let FilesView::Ready { shown, .. } = files_view(&f, &mut vcx) else {
        panic!("listed");
    };
    let FileDiff::Rows(rows) = &shown.as_ref().unwrap().diff else {
        panic!("read whole: {shown:?}");
    };
    let text: Vec<&str> = rows.rows().iter().map(|row| row.content.as_str()).collect();
    assert!(text.contains(&"old") && text.contains(&"new"), "{text:?}");

    f.ws.update(&mut vcx, |ws, cx| ws.show_pr_file(id, "logo.bin", cx));
    vcx.run_until_parked();
    let read = vcx.debug_bounds("pr-file-recover").unwrap().center();
    vcx.simulate_click(read, Modifiers::default());
    vcx.run_until_parked();
    let FilesView::Ready { shown, .. } = files_view(&f, &mut vcx) else {
        panic!("listed");
    };
    assert_eq!(
        shown.unwrap().recovery,
        Some(RecoveryView::Failed(RecoverFailure::Binary)),
        "a binary side says so, and can be tried again"
    );
    assert!(vcx.debug_bounds("pr-file-recover").is_some());
}

/// The handoff of selected lines, as prepared.
fn code_handoff(
    f: &super::GitFixture,
    id: PrDetailId,
    cx: &mut TestAppContext,
) -> Option<(
    crate::workspace::pages::pull_requests::detail::handoff::Payload,
    crate::workspace::pages::pull_requests::detail::handoff::Prepared,
)> {
    f.ws.update(cx, |ws, cx| ws.open_pr_code_handoff(id, cx));
    f.ws.read_with(cx, |ws, _| {
        let handoff = ws.pages.pull_requests.detail.as_ref()?.handoff()?;
        Some((handoff.payload.clone(), handoff.prompt.clone()))
    })
}

#[gpui::test]
fn selected_lines_are_handed_over_with_their_reference(cx: &mut TestAppContext) {
    use crate::workspace::pages::pull_requests::detail::handoff::{Payload, Prepared};
    let mut detail = mergeable(7, "feat");
    detail.head_sha = "4f9c2e1abcdef".into();
    let (f, gh, id) = opened(cx, detail);
    gh.set_pr_files(
        &repo(),
        7,
        PrFiles {
            files: vec![file(
                "src/a.rs",
                Some("@@ -1,3 +1,3 @@\n one\n-two\n+two!\n three\n@@ -10,2 +10,2 @@\n ten\n-x\n+y"),
            )],
        },
    );
    select(&f, id, cx);
    assert_eq!(code_handoff(&f, id, cx), None, "nothing selected");

    f.ws.update(cx, |ws, cx| {
        ws.select_pr_line(id, Side::New, 2, false, cx);
        ws.select_pr_line(id, Side::New, 3, true, cx);
    });
    let Some((Payload::SelectedCode(_), Prepared::Ready(prompt))) = code_handoff(&f, id, cx) else {
        panic!("ready at once");
    };
    assert!(prompt.contains("src/a.rs:2-3 (new) @ 4f9c2e1"), "{prompt}");
    assert!(prompt.contains("two!\nthree"), "{prompt}");
    assert!(!prompt.contains("not quoted"), "{prompt}");
    assert_eq!(gh.count(GhOp::FailedLog), 0, "nothing is read");

    // Across the gap between hunks: the lines not in the diff are named.
    f.ws.update(cx, |ws, cx| ws.select_pr_line(id, Side::New, 11, true, cx));
    let Some((_, Prepared::Ready(prompt))) = code_handoff(&f, id, cx) else {
        panic!("ready");
    };
    assert!(prompt.contains("src/a.rs:2-11 (new)"), "{prompt}");
    assert!(prompt.contains("three\nten\ny"), "{prompt}");
    assert!(
        prompt.contains("Lines outside the diff are not quoted"),
        "{prompt}"
    );
}

/// Lines selected otherwise after the handoff opened: it says so, Insert
/// is off, and inserting anyway closes it with a warning.
#[gpui::test]
fn a_changed_selection_is_not_inserted(cx: &mut TestAppContext) {
    use gpui::VisualTestContext;
    let mut detail = mergeable(7, "feat");
    detail.head_sha = "4f9c2e1abcdef".into();
    let (f, gh, id) = opened(cx, detail);
    gh.set_pr_files(
        &repo(),
        7,
        PrFiles {
            files: vec![file(
                "src/a.rs",
                Some("@@ -1,3 +1,3 @@\n one\n-two\n+two!\n three"),
            )],
        },
    );
    select(&f, id, cx);
    f.ws.update(cx, |ws, cx| ws.select_pr_line(id, Side::New, 2, false, cx));
    assert!(code_handoff(&f, id, cx).is_some());
    let chosen = |f: &super::GitFixture, cx: &mut TestAppContext| {
        f.ws.update(cx, |ws, cx| {
            ws.stage_pr_page(cx);
            ws.pages
                .pull_requests
                .detail_snapshot
                .as_ref()
                .and_then(|snap| snap.handoff.clone())
        })
    };
    assert_eq!(chosen(&f, cx).unwrap().blocked, None);

    f.ws.update(cx, |ws, cx| ws.select_pr_line(id, Side::New, 3, false, cx));
    assert_eq!(
        chosen(&f, cx).unwrap().blocked,
        Some(crate::workspace::pages::pull_requests::detail::view::HandoffBlock::SelectionChanged)
    );
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| ws.insert_pr_handoff(id, window, cx));
    });
    cx.run_until_parked();
    assert!(chosen(&f, cx).is_none(), "closed");
    assert!(f.ws.read_with(cx, |ws, _| {
        ws.error_history()
            .iter()
            .any(|r| r.dedup_key.as_deref() == Some("github.handoff.selection"))
    }));
}

#[gpui::test]
fn sending_the_same_lines_again_keeps_the_handoff(cx: &mut TestAppContext) {
    let (f, gh, id) = opened(cx, mergeable(7, "feat"));
    gh.set_pr_files(
        &repo(),
        7,
        PrFiles {
            files: vec![file("src/a.rs", Some(PATCH))],
        },
    );
    select(&f, id, cx);
    f.ws.update(cx, |ws, cx| ws.select_pr_line(id, Side::Old, 2, false, cx));
    let Some((_, crate::workspace::pages::pull_requests::detail::handoff::Prepared::Ready(prompt))) =
        code_handoff(&f, id, cx)
    else {
        panic!("ready");
    };
    assert!(
        prompt.contains("a.rs:2 (old)") && prompt.contains("on the base side"),
        "{prompt}"
    );
    let before = f.ws.read_with(cx, |ws, _| {
        ws.pages
            .pull_requests
            .detail
            .as_ref()
            .unwrap()
            .handoff()
            .cloned()
    });
    assert!(code_handoff(&f, id, cx).is_some());
    let after = f.ws.read_with(cx, |ws, _| {
        ws.pages
            .pull_requests
            .detail
            .as_ref()
            .unwrap()
            .handoff()
            .cloned()
    });
    assert_eq!(before, after);
}
