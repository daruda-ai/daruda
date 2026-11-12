//! GitHub pull requests as a lane sees them: which repository a remote names
//! ([`remote_parse`]), which pull request a branch belongs to ([`link`]),
//! and what its checks add up to ([`checks`]).
//!
//! GPUI-free. Every GitHub request goes through the `gh` CLI, so this crate
//! stores no token and speaks no HTTP itself.

pub mod checks;
pub mod link;
pub mod model;
pub mod remote_parse;

pub use model::{
    ActionsJob, CheckRollup, CheckRun, CheckStatus, FailedCheck, PrState, PrSummary, RepoId,
    ReviewDecision,
};
