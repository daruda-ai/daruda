//! Whether this process may reach the developer's real machine: flush to
//! disk, read another program's credentials.
//!
//! A `test-support` build starts isolated, so no test needs to opt in. The
//! feature reaches dev-dependencies only, but `cargo test` links them into
//! the app binary it builds as well, so every shipping binary's `main` calls
//! [`require_production`] first. Without the feature nothing is isolated.

/// Cleared by [`require_production`]; read only in test-support builds.
#[cfg(feature = "test-support")]
static ISOLATED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// End isolation for this process. Every shipping binary calls this first.
pub fn require_production() {
    #[cfg(feature = "test-support")]
    ISOLATED.store(false, std::sync::atomic::Ordering::Release);
}

/// True while a test-support build has not required production.
pub fn isolated() -> bool {
    #[cfg(feature = "test-support")]
    {
        ISOLATED.load(std::sync::atomic::Ordering::Acquire)
    }
    #[cfg(not(feature = "test-support"))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This crate's unit tests enable `test-support` through its
    /// self-dev-dependency, so they start isolated like every other test.
    #[test]
    fn unit_tests_start_isolated() {
        assert!(isolated());
    }
}
