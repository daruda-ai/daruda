//! daruda patch: host row decorations (`LineDecoration`) replace the editor's
//! sequential line numbers with host gutter labels, size the gutter to the
//! widest label, and paint a full-width row background.
#![cfg(feature = "test-support")]

#[cfg(feature = "gpui-fast")]
use gpui_fast as gpui;

use gpui::{
    AppContext as _, Background, Context, Entity, Hsla, IntoElement, ParentElement as _, Pixels,
    Render, Styled as _, TestAppContext, Window, div, hsla, px, size,
};
use gpui_base::{
    input::{Editor, EditorState, LineDecoration},
    test_support::{begin_frame, texts},
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

struct Case {
    name: &'static str,
    /// Per buffer row: gutter label and background. Rows past the end keep
    /// their sequential number.
    rows: &'static [(Option<&'static str>, Option<fn() -> Hsla>)],
    /// The gutter width in columns: the widest label, else three digits.
    columns: usize,
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
            assert!(
                fill.top() <= text.top() && text.bottom() <= fill.bottom(),
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
        },
    ];

    let mut painted: Vec<Painted> = cases.iter().map(|case| render_case(cx, case)).collect();
    painted.sort_by_key(|painted| painted.columns);
    for pair in painted.windows(2) {
        assert!(
            pair[0].text_origin < pair[1].text_origin,
            "{} ({} columns) text starts at {:?}, not left of {} ({} columns) at {:?}",
            pair[0].name,
            pair[0].columns,
            pair[0].text_origin,
            pair[1].name,
            pair[1].columns,
            pair[1].text_origin,
        );
    }
}
