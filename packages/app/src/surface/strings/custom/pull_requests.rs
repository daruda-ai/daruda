//! Pull requests strings that format an argument first.

use chrono::{DateTime, Utc};
use daruda_github::availability::{CredentialSource, GhAccount};

fn time_of_day(at: DateTime<Utc>) -> String {
    let pattern = crate::surface::strings::timestamp::time_of_day();
    at.with_timezone(&chrono::Local)
        .format(&pattern)
        .to_string()
}

/// A spent rate limit, with the local time GitHub reported it resets.
pub(crate) fn rate_limited_reported(at: DateTime<Utc>) -> String {
    rust_i18n::t!(
        "pull_requests.rate_limited_reported",
        time = time_of_day(at)
    )
    .into_owned()
}

/// A spent rate limit with no reported reset, with the guessed local time
/// the list is asked again.
pub(crate) fn rate_limited_estimated(at: DateTime<Utc>) -> String {
    rust_i18n::t!(
        "pull_requests.rate_limited_estimated",
        time = time_of_day(at)
    )
    .into_owned()
}

/// Each shown host's account and where gh keeps its token, one clause per
/// host: which identity searched, for a result that looks incomplete.
pub(crate) fn accounts(accounts: &[(String, GhAccount)]) -> String {
    accounts
        .iter()
        .map(|(host, account)| {
            use crate::surface::strings::pull_requests as s;
            let source = match &account.source {
                CredentialSource::Keyring => s::source_keyring(),
                CredentialSource::Env(name) => s::source_env(name),
                CredentialSource::ConfigFile => s::source_config(),
                CredentialSource::Other(source) => source.clone(),
            };
            rust_i18n::t!(
                "pull_requests.accounts",
                host = host,
                login = account.login,
                source = source
            )
            .into_owned()
        })
        .collect::<Vec<_>>()
        .join(" · ")
}
