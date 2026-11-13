//! A lane's GitHub pull request: which one it is ([`link`]), kept current by
//! the process-wide [`crate::github::PrPoller`]; and the window's pull request
//! list ([`pr_list`]), kept by [`crate::github::PrList`].

pub(crate) mod create;
pub(crate) mod create_modal;
pub(in crate::workspace) mod lane_pr;
pub(crate) mod link;
pub(crate) mod pr_list;
mod review_send;
#[cfg(feature = "screenshot")]
pub(in crate::workspace) mod screenshot;
pub(crate) mod strip;
pub(crate) mod tone;
