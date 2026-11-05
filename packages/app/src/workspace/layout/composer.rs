//! Pixel budget shared by the bottom input's viewport and dock floor.

pub(in crate::workspace) struct ComposerLayout {
    pub(in crate::workspace) max_rows: usize,
    fixed_height: f32,
    row_height: f32,
}

impl ComposerLayout {
    pub(in crate::workspace) fn new(
        max_height: f32,
        fixed_height: f32,
        row_height: f32,
        requested_rows: usize,
    ) -> Self {
        let available_rows = ((max_height - fixed_height).max(0.0) / row_height).floor() as usize;
        Self {
            max_rows: requested_rows.min(available_rows).max(1),
            fixed_height,
            row_height,
        }
    }

    pub(in crate::workspace) fn height(&self, rows: usize) -> f32 {
        self.fixed_height + rows.clamp(1, self.max_rows) as f32 * self.row_height
    }
}

#[cfg(test)]
mod tests;
