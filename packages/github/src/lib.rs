//! GitHub pull requests as a lane sees them: which repository a remote names
//! ([`remote_parse`]), which pull request a branch belongs to ([`link`]),
//! and what its checks add up to ([`CheckRollup`]).
//!
//! GPUI-free. Every GitHub request goes through the `gh` CLI behind the
//! [`Gh`] trait, so this crate stores no token and speaks no HTTP itself.
//! Tests use `FakeGh` (feature `test-support`) and never spawn `gh`.

mod actions;
pub mod address;
mod auto_merge;
pub mod availability;
mod backend;
mod checkout;
mod checks;
mod error;
#[cfg(any(test, feature = "test-support"))]
pub mod fake;
mod files;
pub mod link;
pub mod model;
pub mod prompt;
mod query;
pub mod remote_parse;
mod search;
mod system;

pub use actions::MergeBlock;
pub use auto_merge::enable_auto_merge_shown;
pub use backend::Gh;
pub use checkout::PrCheckout;
pub use error::{FailureKind, GhError};
pub use files::{
    FileChange, FileText, PR_FILES_MAX, PR_FILES_PER_PAGE, PrFile, PrFiles, PrFilesPage,
};
pub use model::{
    ActionsJob, AutoMergeOn, BlockReason, CheckEntry, CheckKind, CheckRollup, CheckRun,
    CheckStatus, Conversation, DiffSide, FailedCheck, GITHUB_HOST, LatestReview, LatestReviewState,
    MergeMethod, Mergeability, NewPr, Permission, PrDetail, PrListItem, PrSearch, PrState,
    PrSummary, QueueEntry, QueueState, RepoId, ReviewComment, ReviewDecision, ReviewEvent,
    ReviewThread, ReviewThreadDraft, ReviewThreads, ReviewerState, SubmittedReview,
    ThreadPermissions, ViewerCan, reviewer_name_ok,
};
pub use query::{PrStack, RepoPrs, StackEntry, StackRead};
pub use search::{
    PrAudience, PrCommitStatus, PrRequestedReviewer, PrReviewFilter, PrSearchContinuation,
    PrSearchCriteria, PrSearchPage, PrSearchRequest, PrSearchState,
};
pub use system::SystemGh;
