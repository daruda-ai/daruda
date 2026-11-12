//! A lane's GitHub pull request: which one it is ([`link`]), kept current by
//! the process-wide [`crate::github::PrPoller`].

pub(crate) mod create;
pub(crate) mod create_modal;
pub(crate) mod link;
#[cfg(feature = "screenshot")]
pub(in crate::workspace) mod screenshot;
pub(crate) mod strip;
pub(crate) mod tone;
