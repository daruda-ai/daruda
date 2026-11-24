use gpui::{
    AbsoluteLength, App, Font, Hsla, Pixels, ShapedLine, SharedString, TextRun, TextStyle, Window,
    px,
};
use gpui_base::input::EditorLineNumberRenderer;

use crate::ActiveTheme as _;

/// Optional line-number presentation supplied by the application or UI layer.
///
/// Unspecified properties inherit the editor's defaults. Keeping them optional
/// lets a per-editor override follow later theme changes for every other property.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GutterStyle {
    gap: Option<AbsoluteLength>,
    line_number_color: Option<Hsla>,
    active_line_number_color: Option<Hsla>,
    line_number_font_family: Option<SharedString>,
    line_number_font_size: Option<AbsoluteLength>,
}

impl EditorLineNumberRenderer for GutterStyle {
    fn measure_line_numbers(
        &self,
        digit_count: usize,
        body_style: &TextStyle,
        window: &Window,
    ) -> (Pixels, Pixels) {
        let gap = self.resolve_gap(window.rem_size());
        // Hidden line numbers need spacing but no font resolution or glyph metrics.
        if digit_count == 0 {
            return (px(0.), gap);
        }
        let style = self.resolve_text_style(body_style);
        let font_size = style.font_size.to_pixels(window.rem_size());
        let font_id = window.text_system().resolve_font(&style.font());
        // A font's punctuation does not bound its digits, and proportional
        // digits can differ from each other. Every supported digit must fit.
        let width = "0123456789"
            .chars()
            .map(|digit| window.text_system().layout_width(font_id, font_size, digit))
            .fold(px(0.), Pixels::max)
            * digit_count;
        (width, gap)
    }

    fn resolve_line_number_baseline_offset(
        &self,
        body_style: &TextStyle,
        window: &Window,
    ) -> Pixels {
        let number_style = self.resolve_text_style(body_style);
        let text_system = window.text_system();
        let line_height = window.line_height();
        // Empty rows have no glyph metrics. Use the configured fonts so edits
        // and syntax highlighting cannot move the line-number baseline.
        let baseline = |style: &TextStyle| {
            let size = style.font_size.to_pixels(window.rem_size());
            let font_id = text_system.resolve_font(&style.font());
            let ascent = text_system.ascent(font_id, size);
            // GPUI's shaped layouts use a positive descent for centered painting.
            let descent = text_system.descent(font_id, size).abs();
            (line_height - ascent - descent) / 2. + ascent
        };
        baseline(body_style) - baseline(&number_style)
    }

    fn shape_line_number(
        &self,
        number: usize,
        is_active: bool,
        body_style: &TextStyle,
        window: &Window,
        cx: &App,
    ) -> ShapedLine {
        // Base aligns shaped advances, so padding would distort the label width.
        self.shape_line_label(number.to_string().into(), is_active, body_style, window, cx)
    }

    /// daruda patch: a host gutter label takes the number typography; the
    /// line number above is shaped through here.
    fn shape_line_label(
        &self,
        label: SharedString,
        is_active: bool,
        body_style: &TextStyle,
        window: &Window,
        cx: &App,
    ) -> ShapedLine {
        let color = if is_active {
            self.active_line_number_color
                .unwrap_or(cx.theme().foreground)
        } else {
            self.line_number_color
                .unwrap_or(cx.theme().muted_foreground)
        };
        // Build only the shaping inputs; cloning TextStyle for every visible
        // line would also copy unrelated layout settings and shared handles.
        let run = TextRun {
            len: label.len(),
            font: Font {
                family: self
                    .line_number_font_family
                    .as_ref()
                    .unwrap_or(&body_style.font_family)
                    .clone(),
                features: body_style.font_features.clone(),
                fallbacks: body_style.font_fallbacks.clone(),
                weight: body_style.font_weight,
                style: body_style.font_style,
            },
            color,
            // Body annotations describe editable text, not the gutter labels.
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        window.text_system().shape_line(
            label,
            self.line_number_font_size
                .unwrap_or(body_style.font_size)
                .to_pixels(window.rem_size()),
            &[run],
            None,
        )
    }
}

impl GutterStyle {
    /// Sets the gap before the body text, after any folding controls.
    ///
    /// Accepts rems that follow window zoom or pixels for a fixed gap.
    pub fn with_gap(mut self, gap: impl Into<AbsoluteLength>) -> Self {
        self.gap = Some(gap.into());
        self
    }

    /// Sets the color of line numbers outside the current line.
    pub fn with_line_number_color(mut self, color: Hsla) -> Self {
        self.line_number_color = Some(color);
        self
    }

    /// Sets the color of the current line number.
    pub fn with_active_line_number_color(mut self, color: Hsla) -> Self {
        self.active_line_number_color = Some(color);
        self
    }

    /// Sets the line-number font independently of the body font.
    pub fn with_line_number_font_family(mut self, family: impl Into<SharedString>) -> Self {
        self.line_number_font_family = Some(family.into());
        self
    }

    /// Sets the line-number font size without changing the body row height.
    ///
    /// Accepts rems that follow window zoom or pixels for a fixed font size.
    pub fn with_line_number_font_size(mut self, size: impl Into<AbsoluteLength>) -> Self {
        self.line_number_font_size = Some(size.into());
        self
    }

    /// Returns the explicitly specified gap.
    pub fn gap(&self) -> Option<AbsoluteLength> {
        self.gap
    }

    /// Returns the explicitly specified normal line-number color.
    pub fn line_number_color(&self) -> Option<Hsla> {
        self.line_number_color
    }

    /// Returns the explicitly specified current-line number color.
    pub fn active_line_number_color(&self) -> Option<Hsla> {
        self.active_line_number_color
    }

    /// Returns the explicitly specified line-number font family.
    pub fn line_number_font_family(&self) -> Option<&SharedString> {
        self.line_number_font_family.as_ref()
    }

    /// Returns the explicitly specified line-number font size.
    pub fn line_number_font_size(&self) -> Option<AbsoluteLength> {
        self.line_number_font_size
    }

    /// Applies only explicitly specified properties, including transparent colors.
    pub(crate) fn apply_overrides(&mut self, overrides: Self) {
        self.gap = overrides.gap.or(self.gap);
        self.line_number_color = overrides.line_number_color.or(self.line_number_color);
        self.active_line_number_color = overrides
            .active_line_number_color
            .or(self.active_line_number_color);
        if let Some(family) = overrides.line_number_font_family {
            self.line_number_font_family = Some(family);
        }
        self.line_number_font_size = overrides
            .line_number_font_size
            .or(self.line_number_font_size);
    }

    pub(crate) fn resolve_gap(&self, rem_size: Pixels) -> Pixels {
        // Preserve each length's unit so GPUI controls pixel and rem behavior.
        self.gap.unwrap_or(px(6.).into()).to_pixels(rem_size)
    }

    pub(crate) fn resolve_text_style(&self, body: &TextStyle) -> TextStyle {
        let mut style = body.clone();
        if let Some(family) = &self.line_number_font_family {
            style.font_family = family.clone();
        }
        if let Some(size) = self.line_number_font_size {
            style.font_size = size;
        }
        style
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Styled as _, div, px, rems, rgb};

    #[test]
    fn test_gutter_style_builder_and_partial_overrides() {
        let normal = rgb(0x808080).into();
        let active = rgb(0xeeeeee).into();
        let mut style = GutterStyle::default()
            .with_gap(px(12.))
            .with_line_number_color(normal)
            .with_active_line_number_color(active)
            .with_line_number_font_family("Number Font")
            .with_line_number_font_size(px(12.));
        let defaults = style.clone();

        style.apply_overrides(GutterStyle::default());
        assert_eq!(style, defaults);

        // Zero spacing and transparent ink are explicit values, so treating
        // either as an unset sentinel would lose an application's override.
        style.apply_overrides(
            GutterStyle::default()
                .with_gap(px(0.))
                .with_line_number_color(Hsla::transparent_black())
                .with_line_number_font_size(px(10.)),
        );
        assert_eq!(style.gap(), Some(px(0.).into()));
        assert_eq!(style.line_number_color(), Some(Hsla::transparent_black()));
        assert_eq!(style.active_line_number_color(), Some(active));
        assert_eq!(
            style.line_number_font_family().unwrap().as_ref(),
            "Number Font"
        );
        assert_eq!(style.line_number_font_size(), Some(px(10.).into()));

        style.apply_overrides(GutterStyle::default().with_active_line_number_color(normal));
        assert_eq!(style.active_line_number_color(), Some(normal));
        assert_eq!(style.line_number_color(), Some(Hsla::transparent_black()));
    }

    #[test]
    fn gutter_typography_inherits_unspecified_properties_without_changing_the_body() {
        let mut body = TextStyle::default();
        body.font_family = "Body Font".into();
        body.font_size = px(16.).into();
        let original = body.clone();

        assert_eq!(GutterStyle::default().resolve_text_style(&body), original);
        assert_eq!(GutterStyle::default().resolve_gap(px(16.)), px(6.));

        let family_only = GutterStyle::default()
            .with_line_number_font_family("Number Font")
            .resolve_text_style(&body);
        assert_eq!(family_only.font_family.as_ref(), "Number Font");
        assert_eq!(family_only.font_size, body.font_size);
        assert_eq!(family_only.line_height, body.line_height);

        let size_only = GutterStyle::default()
            .with_line_number_font_size(px(12.))
            .resolve_text_style(&body);
        assert_eq!(size_only.font_family, body.font_family);
        assert_eq!(size_only.font_size.to_pixels(px(16.)), px(12.));
        assert_eq!(size_only.line_height, body.line_height);
        assert_eq!(body, original);
    }

    #[test]
    fn gutter_dimensions_follow_zoom_without_changing_configured_values() {
        let style = GutterStyle::default()
            .with_gap(rems(0.75))
            .with_line_number_font_size(rems(0.75));
        let fixed_style = GutterStyle::default()
            .with_gap(px(12.))
            .with_line_number_font_size(px(12.));
        let mut body = TextStyle::default();
        body.font_size = rems(1.).into();
        let resolved = style.resolve_text_style(&body);
        let reference_gap = div().gap_3().style().gap.width.unwrap();

        for (rem_size, expected_size) in [(12., 9.), (16., 12.), (24., 18.), (32., 24.), (16., 12.)]
        {
            assert_eq!(style.resolve_gap(px(rem_size)), px(expected_size));
            assert_eq!(fixed_style.resolve_gap(px(rem_size)), px(12.));
            assert_eq!(
                fixed_style
                    .resolve_text_style(&body)
                    .font_size
                    .to_pixels(px(rem_size)),
                px(12.)
            );
            assert_eq!(
                style.resolve_gap(px(rem_size)),
                reference_gap.to_pixels(px(0.).into(), px(rem_size))
            );
            assert_eq!(
                resolved.font_size.to_pixels(px(rem_size)),
                px(expected_size)
            );
            assert_eq!(GutterStyle::default().resolve_gap(px(rem_size)), px(6.));
            assert_eq!(
                style.clone().with_gap(px(0.)).resolve_gap(px(rem_size)),
                px(0.)
            );
            assert_eq!(GutterStyle::default().resolve_text_style(&body), body);
        }
        assert_eq!(style.gap(), Some(rems(0.75).into()));
        assert_eq!(style.line_number_font_size(), Some(rems(0.75).into()));
    }

    /// daruda patch: host gutter labels (`LineDecoration::gutter`) through the
    /// shipped `Input`, whose editor shapes them with `GutterStyle`.
    mod line_labels {
        use gpui::{
            AppContext as _, Background, Bounds, Context, Entity, Hsla, IntoElement,
            ParentElement as _, Render, ScaledPixels, TestAppContext, Window, hsla,
        };
        use gpui_base::test_support::{begin_frame, texts};

        use super::*;
        use crate::input::{EditorState, Input, LineDecoration};

        const TEXT: &str = "alpha\nbeta\ngamma\ndelta";

        fn add_bg() -> Hsla {
            hsla(0.33, 1.0, 0.5, 1.0)
        }

        fn del_bg() -> Hsla {
            hsla(0.0, 1.0, 0.5, 1.0)
        }

        struct Frame {
            editor: Entity<EditorState>,
            style: GutterStyle,
        }

        impl Render for Frame {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div()
                    .size_full()
                    .child(Input::from_state(self.editor.clone()).gutter_style(self.style.clone()))
            }
        }

        /// Per buffer row: gutter label and background; later rows keep their
        /// sequential number. The base crate's `tests/line_decorations.rs`
        /// pins the same cases through the plain `Editor`.
        type Rows = &'static [(Option<&'static str>, Option<fn() -> Hsla>)];

        struct Label {
            /// The painted text, trimmed.
            text: String,
            /// Where the gutter text was painted, in the quads' scaled pixels.
            bounds: Bounds<ScaledPixels>,
        }

        struct Painted {
            /// The gutter labels, in row order.
            labels: Vec<Label>,
            /// Per decorated row, its painted background quad.
            fills: Vec<Option<Bounds<ScaledPixels>>>,
            text_origin: Pixels,
        }

        fn render(cx: &mut TestAppContext, name: &str, rows: Rows, style: GutterStyle) -> Painted {
            let (frame, vcx) = cx.add_window_view(|window, cx| Frame {
                editor: cx.new(|cx| EditorState::new(window, cx).default_value(TEXT)),
                style,
            });
            let editor = frame.read_with(vcx, |frame, _| frame.editor.clone());
            let decorations = rows
                .iter()
                .map(|(gutter, background)| LineDecoration {
                    background: background.map(|color| color()),
                    gutter: gutter.map(Into::into),
                })
                .collect();
            editor.update(vcx, |state, cx| state.set_line_decorations(decorations, cx));
            vcx.run_until_parked();

            let (labels, fills) = vcx.update(|window, cx| {
                // No Root starts the frame's text record, so start it here.
                begin_frame(window);
                window.draw(cx).clear(cx);
                let scale = window.scale_factor();
                let labels = texts(window)
                    .iter()
                    .map(|snapshot| Label {
                        text: snapshot.text().trim().to_string(),
                        bounds: snapshot.bounds().scale(scale),
                    })
                    .collect();
                let quads = window.painted_quads();
                let fills = rows
                    .iter()
                    .enumerate()
                    .map(|(row, (_, background))| {
                        let color: Background = background.as_ref()?().into();
                        let matching: Vec<_> = quads
                            .iter()
                            .filter(|quad| quad.background == color)
                            .map(|quad| quad.bounds)
                            .collect();
                        assert_eq!(matching.len(), 1, "{name}: row {row} background quads");
                        Some(matching[0])
                    })
                    .collect();
                (labels, fills)
            });
            let text_origin = editor.read_with(vcx, |state, _| state.text_origin_offset().unwrap());
            Painted {
                labels,
                fills,
                text_origin,
            }
        }

        #[gpui::test]
        fn gutter_style_shapes_host_labels_and_sizes_the_gutter(cx: &mut TestAppContext) {
            cx.update(crate::init);
            // (name, rows, gutter columns: the widest label, widened to the
            // line count's digits, at least three, when any row keeps its number)
            let cases: [(&str, Rows, usize); 6] = [
                ("undecorated", &[], 3),
                // The diff viewer's packed "old new" pair; a hunk header is
                // blank and the undecorated last row keeps its own number.
                (
                    "diff pair",
                    &[
                        (Some(" 9 10"), Some(add_bg)),
                        (Some("10   "), Some(del_bg)),
                        (Some("     "), None),
                    ],
                    5,
                ),
                (
                    "wide pair",
                    &[
                        (Some("1000 1000"), None),
                        (Some("1001     "), Some(del_bg)),
                        (Some("     1001"), Some(add_bg)),
                        (Some("1002 1002"), None),
                    ],
                    9,
                ),
                // Partial labels: rows past the decorations keep their
                // sequential number, so even an empty label leaves the line
                // count's width.
                ("partial labels", &[(Some(""), Some(add_bg))], 3),
                // An unlabeled row inside the decorations also keeps its number.
                (
                    "unlabeled row",
                    &[
                        (Some("x"), None),
                        (Some("y"), None),
                        (None, Some(del_bg)),
                        (Some("z"), None),
                    ],
                    3,
                ),
                // Hidden numbers: empty labels reserve no number columns.
                (
                    "hidden numbers",
                    &[
                        (Some(""), None),
                        (Some(""), None),
                        (Some(""), None),
                        (Some(""), None),
                    ],
                    0,
                ),
            ];

            let mut origins = Vec::new();
            for (name, rows, columns) in cases {
                let painted = render(cx, name, rows, GutterStyle::default());
                let expected: Vec<String> = (0..TEXT.lines().count())
                    .map(|row| match rows.get(row).and_then(|(gutter, _)| *gutter) {
                        Some(label) => label.trim().to_string(),
                        None => (row + 1).to_string(),
                    })
                    .collect();
                let text: Vec<&str> = painted.labels.iter().map(|l| l.text.as_str()).collect();
                assert_eq!(text, expected, "{name}: gutter text");

                // With the body's typography the label sits on its row, so the
                // row's background spans it vertically.
                for (row, fill) in painted.fills.iter().enumerate() {
                    let Some(fill) = fill else { continue };
                    let text = painted.labels[row].bounds;
                    // Quad edges snap to device pixels; recorded text bounds do not.
                    let slack = ScaledPixels(0.5);
                    assert!(
                        fill.top() <= text.top() + slack && text.bottom() <= fill.bottom() + slack,
                        "{name}: row {row} background {:?}..{:?} does not span its gutter \
                         text {:?}..{:?}",
                        fill.top(),
                        fill.bottom(),
                        text.top(),
                        text.bottom(),
                    );
                }

                // Labels take the number typography: a larger number font
                // widens every label. (Its baseline correction moves the
                // label off the row top, so only the width is compared.)
                let large = render(
                    cx,
                    name,
                    rows,
                    GutterStyle::default().with_line_number_font_size(px(32.)),
                );
                for (row, (label, large)) in painted.labels.iter().zip(&large.labels).enumerate() {
                    if label.text.is_empty() {
                        continue;
                    }
                    let (width, large_width) = (label.bounds.size.width, large.bounds.size.width);
                    assert!(
                        width < large_width,
                        "{name}: row {row} label {:?} is {large_width:?} wide in a 32px \
                         number font, not wider than {width:?}",
                        label.text,
                    );
                }
                origins.push((columns, name, painted.text_origin));
            }

            // Equal column counts give equal gutters; more columns start the
            // text further right.
            origins.sort_by_key(|(columns, ..)| *columns);
            for pair in origins.windows(2) {
                let ((narrow, narrow_name, narrow_origin), (wide, wide_name, wide_origin)) =
                    (pair[0], pair[1]);
                let ordered = if narrow == wide {
                    narrow_origin == wide_origin
                } else {
                    narrow_origin < wide_origin
                };
                assert!(
                    ordered,
                    "{narrow_name} ({narrow} columns) text starts at {narrow_origin:?}; \
                     {wide_name} ({wide} columns) at {wide_origin:?}",
                );
            }
        }
    }
}
