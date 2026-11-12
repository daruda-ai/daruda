//! GitHub pull requests as a lane sees them: which repository a remote names
//! ([`remote_parse`]), which pull request a branch belongs to ([`link`]),
//! and what its checks add up to ([`checks`]).
//!
//! GPUI-free. Every GitHub request goes through the `gh` CLI behind the
//! [`Gh`] trait, so this crate stores no token and speaks no HTTP itself.
//! Tests use `FakeGh` (feature `test-support`) and never spawn `gh`.

pub mod availability;
mod backend;
pub mod checks;
mod error;
#[cfg(any(test, feature = "test-support"))]
pub mod fake;
pub mod link;
pub mod model;
pub mod prompt;
pub mod query;
pub mod remote_parse;
mod system;

pub use backend::Gh;
pub use error::{FailureKind, GhError};
pub use model::{
    ActionsJob, CheckRollup, CheckRun, CheckStatus, FailedCheck, NewPr, PrState, PrSummary, RepoId,
    ReviewComment, ReviewDecision, ReviewThread, ReviewThreads,
};
pub use system::SystemGh;
