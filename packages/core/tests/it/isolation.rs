//! Public API tests for the test-isolation switch.

use daruda_core::isolation::{isolated, require_production};

/// A test-support build stays isolated until a shipping binary demands
/// production; both steps share one test because the switch is process-global.
#[test]
fn a_test_build_is_isolated_until_production_is_required() {
    assert!(isolated(), "a test-support build starts isolated");
    require_production();
    assert!(!isolated(), "a binary that requires production is not");
}
