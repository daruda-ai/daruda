//! daruda patch: host row decorations (`LineDecoration`) replace the editor's
//! sequential line numbers with host gutter labels, size the gutter to cover
//! the widest label and any row's fallback number, and paint a full-width row
//! background that follows its row below an inline completion's ghost lines.
#![cfg(feature = "test-support")]

#[cfg(feature = "gpui-fast")]
use gpui_fast as gpui;

use std::{rc::Rc, time::Duration};

use gpui::{
    App, AppContext as _, Background, Context, Entity, Hsla, IntoElement, ParentElement as _,
    Pixels, Render, ScaledPixels, Styled as _, Task, TestAppContext, Window, div, hsla, px, size,
};
use gpui_base::{
    input::{CompletionProvider, Editor, EditorState, LineDecoration, Position, Rope},
    test_support::{begin_frame, texts},
};
use lsp_types::{
    CompletionContext, CompletionResponse, InlineCompletionContext, InlineCompletionItem,
    InlineCompletionResponse,
};

struct Frame(Entity<EditorState>);

impl Render for Frame {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(Editor::new(&self.0))
    }
}

const TEXT: &str = "alpha\nbeta\ngamma\ndelta";

fn add_bg() -> Hsla {
    hsla(0.33, 1.0, 0.5, 1.0)
}

fn del_bg() -> Hsla {
    hsla(0.0, 1.0, 0.5, 1.0)
}

/// Suggests its text as the inline completion wherever the user types.
struct Suggest(&'static str);

impl CompletionProvider for Suggest {
    fn completions(
        &self,
        _: &Rope,
        _: usize,
        _: CompletionContext,
        _: &mut Window,
        _: &mut App,
    ) -> Task<anyhow::Result<CompletionResponse>> {
        Task::ready(Ok(CompletionResponse::Array(vec![])))
    }

    fn inline_completion(
        &self,
        _: &Rope,
        _: usize,
        _: InlineCompletionContext,
        _: &mut Window,
        _: &mut App,
    ) -> Task<anyhow::Result<InlineCompletionResponse>> {
        Task::ready(Ok(InlineCompletionResponse::Array(vec![
            InlineCompletionItem {
                insert_text: self.0.into(),
                filter_text: None,
                range: None,
                command: None,
                insert_text_format: None,
            },
        ])))
    }

    fn is_completion_trigger(&self, _: usize, _: &str, _: &mut App) -> bool {
        false
    }
}

struct Case {
    name: &'static str,
    /// Per buffer row: gutter label and background. Rows past the end keep
    /// their sequential number.
    rows: &'static [(Option<&'static str>, Option<fn() -> Hsla>)],
    /// The gutter width in columns: the widest label, widened to the line
    /// count's digits (at least three) when any row keeps its number.
    columns: usize,
    /// An inline completion suggested after typing at the end of the first
    /// row; its lines after the first are ghost lines below that row.
    completion: Option<&'static str>,
}

struct Painted {
    name: &'static str,
    columns: usize,
    text_origin: Pixels,
}

fn render_case(cx: &mut TestAppContext, case: &Case) -> Painted {
    let mut editor = None;
    let window = cx.open_window(size(px(600.), px(300.)), |window, cx| {
        let state = cx.new(|cx| EditorState::new(window, cx).default_value(TEXT));
        editor = Some(state.clone());
        Frame(state)
    });
    let editor = editor.unwrap();
    let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);
    let decorations: Vec<LineDecoration> = case
        .rows
        .iter()
        .map(|(gutter, background)| LineDecoration {
            background: background.map(|color| color()),
            gutter: gutter.map(Into::into),
        })
        .collect();
    editor.update(&mut vcx, |state, cx| {
        state.set_line_decorations(decorations, cx)
    });
    vcx.run_until_parked();

    if let Some(completion) = case.completion {
        let first_row_end = TEXT.lines().next().unwrap().len() as u32;
        vcx.update(|window, cx| {
            editor.update(cx, |state, cx| {
                state.lsp_mut().completion_provider = Some(Rc::new(Suggest(completion)));
                state.set_cursor_position(Position::new(0, first_row_end), window, cx);
            });
            // Focus registers the input handler that typing goes through.
            window.draw(cx).clear(cx);
        });
        vcx.simulate_input("a");
        // Past the inline completion's debounce.
        vcx.executor().advance_clock(Duration::from_secs(1));
        vcx.run_until_parked();
    }

    vcx.update(|window, cx| {
        // No Base Root resets the recorded text, so start the frame here.
        begin_frame(window);
        window.draw(cx).clear(cx);

        let name = case.name;
        let snapshots = texts(window);
        let painted: Vec<String> = snapshots
            .iter()
            .map(|snapshot| snapshot.text().trim().to_string())
            .collect();
        let expected: Vec<String> = (0..TEXT.lines().count())
            .map(
                |row| match case.rows.get(row).and_then(|(gutter, _)| *gutter) {
                    Some(label) => label.trim().to_string(),
                    None => (row + 1).to_string(),
                },
            )
            .collect();
        assert_eq!(painted, expected, "{name}: gutter text");

        // A row's background spans that row's gutter text vertically, so a
        // background painted a row off, or at a constant offset, fails here.
        let quads = window.painted_quads();
        let scale = window.scale_factor();
        for (row, (_, background)) in case.rows.iter().enumerate() {
            let Some(color) = background else { continue };
            let color: Background = color().into();
            let matching: Vec<_> = quads
                .iter()
                .filter(|quad| quad.background == color)
                .collect();
            assert_eq!(matching.len(), 1, "{name}: row {row} background quads");
            let fill = matching[0].bounds;
            let text = snapshots[row].bounds().scale(scale);
            // Quad edges snap to device pixels; recorded text bounds do not.
            let slack = ScaledPixels(0.5);
            assert!(
                fill.top() <= text.top() + slack && text.bottom() <= fill.bottom() + slack,
                "{name}: row {row} background {:?}..{:?} does not span its gutter text {:?}..{:?}",
                fill.top(),
                fill.bottom(),
                text.top(),
                text.bottom(),
            );
        }
    });

    let text_origin = editor.read_with(&vcx, |state, _| state.text_origin_offset().unwrap());
    Painted {
        name: case.name,
        columns: case.columns,
        text_origin,
    }
}

#[gpui::test]
fn gutter_labels_replace_line_numbers_and_size_the_gutter(cx: &mut TestAppContext) {
    cx.update(gpui_base::init);
    let cases = [
        // No decorations: sequential numbers in a three-digit gutter.
        Case {
            name: "undecorated",
            rows: &[],
            columns: 3,
            completion: None,
        },
        // The diff viewer's packed "old new" pair; a hunk header is blank and
        // the undecorated last row keeps its own number.
        Case {
            name: "diff pair",
            rows: &[
                (Some(" 9 10"), Some(add_bg)),
                (Some("10   "), Some(del_bg)),
                (Some("     "), None),
            ],
            columns: 5,
            completion: None,
        },
        Case {
            name: "wide pair",
            rows: &[
                (Some("1000 1000"), None),
                (Some("1001     "), Some(del_bg)),
                (Some("     1001"), Some(add_bg)),
                (Some("1002 1002"), None),
            ],
            columns: 9,
            completion: None,
        },
        // Partial labels: rows past the decorations keep their sequential
        // number, so even an empty label leaves the line count's width.
        Case {
            name: "partial labels",
            rows: &[(Some(""), Some(add_bg))],
            columns: 3,
            completion: None,
        },
        // An unlabeled row inside the decorations also keeps its number.
        Case {
            name: "unlabeled row",
            rows: &[
                (Some("x"), None),
                (Some("y"), None),
                (None, Some(del_bg)),
                (Some("z"), None),
            ],
            columns: 3,
            completion: None,
        },
        // Ghost lines below the first row push every later row, and its
        // background, down by their height.
        Case {
            name: "ghost lines",
            rows: &[(None, None), (None, None), (None, Some(add_bg))],
            columns: 3,
            completion: Some("ghost\nghost one\nghost two"),
        },
        // Hidden numbers: empty labels reserve no number columns at all.
        Case {
            name: "hidden numbers",
            rows: &[
                (Some(""), None),
                (Some(""), None),
                (Some(""), None),
                (Some(""), None),
            ],
            columns: 0,
            completion: None,
        },
    ];

    // Equal column counts give equal gutters; more columns start the text
    // further right.
    let mut painted: Vec<Painted> = cases.iter().map(|case| render_case(cx, case)).collect();
    painted.sort_by_key(|painted| painted.columns);
    for pair in painted.windows(2) {
        let (narrow, wide) = (&pair[0], &pair[1]);
        let ordered = if narrow.columns == wide.columns {
            narrow.text_origin == wide.text_origin
        } else {
            narrow.text_origin < wide.text_origin
        };
        assert!(
            ordered,
            "{} ({} columns) text starts at {:?}; {} ({} columns) at {:?}",
            narrow.name,
            narrow.columns,
            narrow.text_origin,
            wide.name,
            wide.columns,
            wide.text_origin,
        );
    }
}
