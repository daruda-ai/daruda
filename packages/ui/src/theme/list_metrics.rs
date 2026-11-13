//! Content-specific column budgets; row chrome belongs to `ui::list_table`.
//! Keep distinct roles separate even when their current widths coincide.

pub const TITLE_MIN_W: f32 = 200.0;
pub const STATUS_W: f32 = 96.0;
pub const PROJECT_W: f32 = 96.0;
pub const AGENT_W: f32 = 84.0;
pub const RELATIVE_TIME_W: f32 = 72.0;
pub const TIMESTAMP_W: f32 = 112.0;
pub const SOURCE_W: f32 = 128.0;
pub const STAGE_W: f32 = 120.0;
pub const ACTIONS_W: f32 = 96.0;
pub const WORKTREE_SCOPE_MAX_W: f32 = 300.0;
/// A pull request's check summary: "Checks passed", "2 failing".
pub const CHECKS_W: f32 = 104.0;
/// A pull request's review decision: "Changes requested" is the longest.
pub const REVIEW_W: f32 = 112.0;
/// A comment glyph and a count.
pub const COMMENTS_W: f32 = 40.0;
/// The worktree working on a pull request.
pub const LANE_W: f32 = 140.0;
