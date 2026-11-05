//! Known failures of the Git executable, independent of a repository.

use super::GitError;

const XCODE_LICENSE_REFUSAL: &str = "You have not agreed to the Xcode license agreements. Please run 'sudo xcodebuild -license' from within a Terminal window to review and agree to the Xcode and Apple SDKs license.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitEnvironmentFailure {
    ExecutableMissing,
    XcodeLicenseNotAccepted,
}

impl GitError {
    /// Only failures that establish that Git itself could not run.
    pub fn environment_failure(&self) -> Option<GitEnvironmentFailure> {
        match self {
            Self::NotFound => Some(GitEnvironmentFailure::ExecutableMissing),
            Self::Exit { stderr, .. }
                if stderr.trim_end_matches(['\r', '\n']) == XCODE_LICENSE_REFUSAL =>
            {
                Some(GitEnvironmentFailure::XcodeLicenseNotAccepted)
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {}
