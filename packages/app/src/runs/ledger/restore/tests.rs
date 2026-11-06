use super::*;

/// A restored flow lapses exactly when it has been open for the whole hold,
/// and never when the proposer gave no time.
#[test]
fn an_open_flow_lapses_once_open_for_the_whole_hold() {
    let hold = u64::try_from(DIRECTORY_HOLD.as_millis()).unwrap();
    assert!(!lapsed(Some(10), 10 + hold - 1));
    assert!(lapsed(Some(10), 10 + hold));
    assert!(!lapsed(None, u64::MAX));
}
