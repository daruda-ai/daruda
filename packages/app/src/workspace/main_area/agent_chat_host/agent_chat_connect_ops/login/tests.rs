use super::login_clears;
use daruda_acp::Remedy;

/// Only an expired login is cleared by signing in. An organization-blocked
/// account is the case that must not reconnect: the login succeeds and changes
/// nothing, so an automatic retry would fail identically while looking like
/// the app is making progress. A pane that never failed is mid-conversation.
#[test]
fn only_a_sign_in_failure_is_cleared_by_signing_in() {
    // (remedy the pane's connection failed with, whether a login clears it)
    let cases = [
        (Some(Remedy::Reauthenticate), true),
        (Some(Remedy::ExternalAction), false),
        (Some(Remedy::Configure), false),
        (Some(Remedy::Retry), false),
        (Some(Remedy::NoneAvailable), false),
        (None, false),
    ];
    for (remedy, cleared) in cases {
        assert_eq!(login_clears(remedy), cleared, "{remedy:?}");
    }
}
