//! Paint immutable SVG bytes; discovery and file I/O never run during rendering.
use super::{Frame, model::LayerColor};
use gpui::{Hsla, IntoElement, Pixels, Styled, canvas};

pub(crate) fn frame(frame: Frame, size: Pixels, color: Hsla) -> gpui::AnyElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, cx| {
            for layer in frame.layers.iter() {
                let color = match layer.color {
                    LayerColor::Theme => color,
                    LayerColor::Fixed(value) => gpui::rgb(value).into(),
                };
                if let Err(error) = window.paint_svg(
                    bounds,
                    layer.key.clone().into(),
                    Some(&layer.svg),
                    Default::default(),
                    color,
                    cx,
                ) {
                    use daruda_store::observability::{
                        error_report::ErrorReport, log_writer::LogWriter,
                    };
                    LogWriter::log(
                        ErrorReport::new("Status icon could not be painted")
                            .message(error.to_string())
                            .at(file!(), line!())
                            .dedup(layer.key.clone())
                            .build(),
                    );
                }
            }
        },
    )
    .size(size)
    .flex_none()
    .into_any_element()
}

#[cfg(test)]
mod tests {
    // Metal rendering, live selection and fallback are covered by
    // workspace::left_dock::projects::agent_badges::tests::visual.
}
