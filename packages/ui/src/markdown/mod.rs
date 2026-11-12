//! Render-once wrapper over `gpui_base::TextView` Markdown.
//!
//! Builds the window-needing `TextView::markdown` inside render so callers can
//! construct Markdown from cx-only snapshot paths. The parse and selection live
//! in GPUI keyed state, a sanctioned MVU exception, unless the host retains them
//! in a [`MarkdownStates`]; either way callers must provide stable ids or live
//! selection resets.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use gpui::{
    AnyElement, App, ElementId, Hsla, IntoElement, ParentElement as _, Pixels, RenderOnce,
    SharedString, Styled as _, Window, div, relative,
};
use gpui_base::TextView;
use gpui_component::ActiveTheme as _;

use crate::{icons, theme};

mod highlight;
mod images;
mod states;
mod style;

pub use states::MarkdownStates;

/// Optional code-block override hook kept domain-free for this UI wrapper.
type CodeBlockRender =
    Arc<dyn Fn(&str, &str, &mut Window, &mut App) -> Option<AnyElement> + Send + Sync>;
type LinkClickHandler = Arc<dyn Fn(&str, &mut Window, &mut App) -> bool + Send + Sync>;
#[cfg(any(test, feature = "test-support"))]
type DebugInlineBounds = Arc<dyn Fn(gpui::Bounds<Pixels>) + Send + Sync>;

#[derive(IntoElement)]
pub struct Markdown {
    id: ElementId,
    text: SharedString,
    selectable: bool,
    color: Option<Hsla>,
    font_family: Option<SharedString>,
    text_size: Option<Pixels>,
    heading_size: Option<Pixels>,
    line_height: Option<f32>,
    full_width: bool,
    surface: Option<Hsla>,
    code_block_render: Option<CodeBlockRender>,
    link_click_handler: Option<LinkClickHandler>,
    retained_in: Option<MarkdownStates>,
    local_images: Option<images::LocalImages>,
    copy_tooltips: CopyTooltips,
    #[cfg(any(test, feature = "test-support"))]
    debug_inline_bounds: Option<DebugInlineBounds>,
}

impl Markdown {
    /// Toggle drag-selection + copy. Default `true` (via [`markdown`]).
    pub fn selectable(mut self, selectable: bool) -> Self {
        self.selectable = selectable;
        self
    }

    /// Fill the container width so block markdown wraps; set false for bubbles.
    pub fn full_width(mut self, full_width: bool) -> Self {
        self.full_width = full_width;
        self
    }

    /// Override the base text color (plain runs). Code blocks keep their own
    /// syntax colors regardless. Defaults to the theme foreground.
    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    /// Override the prose font family. Fenced code keeps its monospace font.
    pub fn font_family(mut self, family: impl Into<SharedString>) -> Self {
        self.font_family = Some(family.into());
        self
    }

    /// Set the body font size so the markdown follows the host's configured
    /// text size instead of the `TextView` default. Headings still scale
    /// relative to their own base. Unset = inherit the ambient size.
    pub fn text_size(mut self, size: Pixels) -> Self {
        self.text_size = Some(size);
        self
    }

    /// Use a fixed heading size in compact reading surfaces. Body text and
    /// heading weights retain their own styling; other callers keep the default scale.
    pub fn heading_size(mut self, size: Pixels) -> Self {
        self.heading_size = Some(size);
        self
    }

    /// Set line height as a multiple of the configured body size.
    pub fn line_height(mut self, line_height: f32) -> Self {
        self.line_height = Some(line_height);
        self
    }

    /// The background this markdown is painted on. Its lightness decides
    /// whether the view's fills and structural lines — inline-code chips, the
    /// code-block fill and border, table lines, the `<hr>` rule — lighten or
    /// darken. Unset leaves that to the UI theme's canvas, which is only the
    /// right surface for a caller painting on it: a pane mirroring the terminal
    /// palette must pass its own (DESIGN.md §AgentChatPane).
    pub fn surface(mut self, surface: Hsla) -> Self {
        self.surface = Some(surface);
        self
    }

    /// Override how fenced code blocks render. The closure receives the fence
    /// language (empty string when none) and raw source, and returns
    /// `Some(element)` to replace the default code rendering or `None` to keep
    /// it. Must be `Send + Sync + 'static` — `TextView` threads it through its
    /// async parse. Domain-free: only `&str` in / `AnyElement` out.
    pub fn code_block_render<F>(mut self, f: F) -> Self
    where
        F: Fn(&str, &str, &mut Window, &mut App) -> Option<AnyElement> + Send + Sync + 'static,
    {
        self.code_block_render = Some(Arc::new(f));
        self
    }

    /// Override link clicks. Return `true` when the link was handled; `false`
    /// keeps TextView's default platform URL opener.
    pub fn link_click_handler<F>(mut self, f: F) -> Self
    where
        F: Fn(&str, &mut Window, &mut App) -> bool + Send + Sync + 'static,
    {
        self.link_click_handler = Some(Arc::new(f));
        self
    }

    /// Enable local image files, optionally resolving relative paths against `base`.
    /// Without a base, only absolute paths and local file URLs are resolved.
    /// Hosts must opt in only for local documents; remote sessions keep their
    /// URI loader. Inline data and HTTP images retain their existing behavior.
    pub fn local_images(mut self, base: Option<&std::path::Path>) -> Self {
        self.local_images = Some(images::LocalImages {
            base: base.map(std::path::Path::to_path_buf),
        });
        self
    }

    /// Keep this body's parse in `states` rather than in keyed state, so it
    /// survives frames in which the element is not drawn — a row a virtualized
    /// list has scrolled out of its overdraw.
    pub fn retained_in(mut self, states: &MarkdownStates) -> Self {
        self.retained_in = Some(states.clone());
        self
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Markdown {
    /// Observe the painted bounds of each paragraph's text (rendered
    /// conformance tests).
    pub fn debug_inline_bounds<F>(mut self, observer: F) -> Self
    where
        F: Fn(gpui::Bounds<Pixels>) + Send + Sync + 'static,
    {
        self.debug_inline_bounds = Some(Arc::new(observer));
        self
    }
}

impl RenderOnce for Markdown {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        // Plain runs inherit the body colour; seed it (caller override else
        // theme foreground) so they never fall back to gpui's default black.
        let color = self.color.unwrap_or_else(|| cx.theme().foreground);
        // Fills and lines lighten or darken from the surface the caller paints
        // on, not the UI canvas: the agent-chat pane mirrors the terminal
        // palette, independent of the UI preset (DESIGN.md §AgentChatPane).
        let surface = self.surface.unwrap_or_else(|| cx.theme().background);
        let style = self.text_view_style(color, surface, window.rem_size(), cx);
        let base_id = self.id.clone();
        let image_text = self.text.clone();
        let view = match &self.retained_in {
            Some(states) => TextView::new(&states.state_for(&self.id, &self.text, cx)),
            None => TextView::markdown(self.id, self.text),
        };
        let mut view = view
            .selectable(self.selectable)
            .style(style)
            .text_color(color);
        if self.surface.is_some() {
            view = view.shared_code_block_highlighter(highlight::for_surface(surface.l >= 0.5, cx));
        }
        if let Some(family) = self.font_family {
            view = view.font_family(family);
        }
        if let Some(size) = self.text_size {
            view = view.text_size(size).line_height(relative(
                self.line_height.unwrap_or(theme::MD_VIEW_LINE_HEIGHT),
            ));
        }
        if let Some(local) = self.local_images {
            let store = match &self.retained_in {
                Some(states) => states.local_image_store(window, cx),
                None => window
                    .use_keyed_state(
                        ElementId::Name(format!("{base_id}-local-image-store").into()),
                        cx,
                        |window, cx| images::ImageStore::new(window, cx),
                    )
                    .read(cx)
                    .clone(),
            };
            let text = image_text;
            let binding = window.use_keyed_state(store.lease_id(&base_id), cx, |_, cx| {
                images::ImageLeaseBinding::new(&store, text.clone(), local.base.clone(), cx)
            });
            let lease = binding.update(cx, |binding, cx| {
                binding.lease(&store, text, local.base.as_deref(), cx)
            });
            let sources = store.sources(local.base, &lease);
            view = view.image_source_with_fallback(move |uri| sources.source(uri));
        }
        if let Some(cbr) = self.code_block_render {
            view = view.code_block_render(move |cb, window, cx| {
                let lang = cb.lang();
                cbr(
                    lang.as_deref().unwrap_or(""),
                    cb.code().as_ref(),
                    window,
                    cx,
                )
            });
        }
        #[cfg(any(test, feature = "test-support"))]
        if let Some(observer) = self.debug_inline_bounds {
            view = view.debug_inline_bounds(move |bounds| observer(bounds));
        }
        let handler = self.link_click_handler;
        view = view.on_link_click(move |url, event, window, cx| {
            // Only a primary click or keyboard activation navigates; a right
            // press belongs to the context menu.
            if let gpui::ClickEvent::Mouse(click) = event
                && click.up.button != gpui::MouseButton::Left
            {
                return;
            }
            if handler
                .as_ref()
                .is_some_and(|handler| handler(url, window, cx))
            {
                return;
            }
            cx.open_url(url);
        });
        // A copy button on every rendered code block. Mermaid fences take the
        // `code_block_render` replace-path above and supply their own. Key each
        // button by the markdown id plus a content hash so its ✓ feedback state
        // stays stable across renders.
        let copy = self.copy_tooltips;
        let copy_surface = self.surface.map(|background| {
            theme::PaneSurfaceTokens::from_background_and_foreground(background, color)
        });
        view = view.code_block_actions(move |cb, window, cx| {
            let mut hasher = DefaultHasher::new();
            cb.code().as_ref().hash(&mut hasher);
            let id = ElementId::Name(format!("{base_id}-codecopy-{}", hasher.finish()).into());
            let button = crate::copy_button(
                id,
                cb.code(),
                icons::icon(icons::COPY),
                copy.copy.clone(),
                icons::icon(icons::CHECK),
                copy.copied.clone(),
                window,
                cx,
            );
            match &copy_surface {
                Some(surface) => crate::button_with_surface(button, surface, cx),
                None => button,
            }
        });
        if !self.full_width {
            return view.into_any_element();
        }
        // Without a width constraint the view lays out at its intrinsic
        // max-content width; fill the container so it wraps. A zero minimum
        // width lets it shrink inside a caller's flex row. Not overflow
        // clipping: that would make this block the content mask that
        // window-wide selection measures drag auto-scroll against, so a drag
        // near the block's own bottom edge would scroll the transcript.
        div()
            .w_full()
            .min_w_0()
            .child(view.w_full())
            .into_any_element()
    }
}

/// The tooltips of the copy button on every code block, in the host's locale.
#[derive(Clone)]
pub struct CopyTooltips {
    pub copy: SharedString,
    pub copied: SharedString,
}

/// Rendered, drag-selectable + copyable Markdown. Selectable by default.
/// `id` must be stable per block across renders (keyed selection state).
pub fn markdown(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    copy_tooltips: CopyTooltips,
) -> Markdown {
    Markdown {
        id: id.into(),
        text: text.into(),
        selectable: true,
        color: None,
        font_family: None,
        text_size: None,
        heading_size: None,
        line_height: None,
        full_width: true,
        surface: None,
        code_block_render: None,
        link_click_handler: None,
        retained_in: None,
        local_images: None,
        copy_tooltips,
        #[cfg(any(test, feature = "test-support"))]
        debug_inline_bounds: None,
    }
}

#[cfg(test)]
mod tests;
