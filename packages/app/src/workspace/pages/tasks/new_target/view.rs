//! The project chooser uses the shared searchable list and modal shell.

use super::NewTaskTarget;
use gpui::{Context, IntoElement, Render, Window, div, prelude::*, px};

impl Render for NewTaskTarget {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .tab_group()
            .debug_selector(|| "task-project-options".into())
            .child(
                crate::ui::list::list(&self.list).max_h(px(crate::ui::theme::PALETTE_MAX_HEIGHT)),
            )
    }
}

#[cfg(test)]
mod tests {
    // Keyboard confirmation is provided by the shared list.
}
