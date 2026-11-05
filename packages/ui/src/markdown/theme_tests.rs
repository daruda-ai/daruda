//! Surface colors must reach the painted table, not just its text.
use crate::test_support::init_gpui_component;
use crate::theme;
use gpui::{
    AppContext as _, Context, IntoElement, Render, TestAppContext, VisualTestContext, Window,
    WindowOptions, div, prelude::*, px,
};

fn copy_tooltips() -> crate::markdown::CopyTooltips {
    crate::markdown::CopyTooltips {
        copy: "Copy".into(),
        copied: "Copied".into(),
    }
}

struct SurfaceProbe {
    background: gpui::Hsla,
    foreground: gpui::Hsla,
}

impl Render for SurfaceProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().bg(self.background)
            .child(div().size(px(8.)).bg(self.background))
            .child(
            super::super::markdown(
                "surface-probe",
                "| Heading | Link |\n| --- | --- |\n| Body | [Open](https://example.invalid) |\n\n- [x] Done",
                copy_tooltips(),
            )
            .surface(self.background)
            .color(self.foreground),
        )
    }
}

#[gpui::test]
fn tables_follow_the_host_surface_under_either_ui_theme(cx: &mut TestAppContext) {
    init_gpui_component(cx);
    for ui in ["daruda_light", "daruda_dark"] {
        cx.update(|cx| assert!(theme::apply_ui_theme(ui, cx)));
        for (background, foreground) in [
            (gpui::rgb(0x1e1e1e).into(), gpui::rgb(0xd0d6e0).into()),
            (gpui::rgb(0xf9fafb).into(), gpui::rgb(0x1a1d24).into()),
        ] {
            let window = cx.update(|cx| {
                cx.open_window(WindowOptions::default(), |_, cx| {
                    cx.new(|_| SurfaceProbe {
                        background,
                        foreground,
                    })
                })
                .unwrap()
            });
            let mut vcx = VisualTestContext::from_window(window.into(), cx);
            vcx.run_until_parked();
            let checkbox_bounds = vcx
                .debug_bounds("markdown-task-checkbox")
                .expect("the checked task paints");
            vcx.update(|window, _| {
                let quads = window.painted_quads();
                let fill = theme::neutral_tint_over(background, theme::MD_VIEW_CODE_FILL_ALPHA);
                assert!(
                    quads.iter().any(|q| {
                        q.background.as_solid() == Some(fill)
                            && q.corner_radii.top_left > gpui::ScaledPixels(0.)
                    }),
                    "table body must use its host fill and retain rounded corners: {ui}"
                );
                let checkbox = quads
                    .iter()
                    .find(|q| {
                        let target = checkbox_bounds.scale(window.scale_factor());
                        q.bounds.contains(&target.center())
                            && (q.bounds.size.width.0 - target.size.width.0).abs() <= 1.
                            && (q.bounds.size.height.0 - target.size.height.0).abs() <= 1.
                            && q.background.as_solid().is_some_and(|c| c.a == 1.)
                    })
                    .expect("the checked task paints its link-colored fill");
                assert!(
                    theme::contrast_ratio(checkbox.background.as_solid().unwrap(), background)
                        >= 4.5,
                    "task color must contrast with its host: {ui}"
                );
                let underlines = window.painted_underlines();
                assert!(!underlines.is_empty(), "the link must paint its underline");
                for underline in underlines {
                    let mut link = underline.color;
                    // The underline is translucent; its hue is the link text's.
                    link.a = 1.;
                    let table_background = background.blend(fill);
                    assert!(
                        theme::contrast_ratio(link, table_background) >= 4.5,
                        "link text must contrast with the composed table fill: {ui}"
                    );
                }
            });
        }
    }
}
