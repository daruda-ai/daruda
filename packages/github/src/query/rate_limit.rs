//! The answer of `gh api rate_limit`: when the GraphQL limit, which every
//! request of this crate draws on, resets once it is spent.

use serde::Deserialize;

use crate::GhError;

#[derive(Deserialize)]
struct RateLimit {
    resources: Resources,
}

#[derive(Deserialize)]
struct Resources {
    graphql: Window,
}

#[derive(Deserialize)]
struct Window {
    remaining: u64,
    /// Unix seconds.
    reset: i64,
}

/// When the spent GraphQL limit resets (Unix seconds); `None` while requests
/// remain, as after a secondary limit, which reports no reset here.
pub(crate) fn parse_graphql_reset(json: &str) -> Result<Option<i64>, GhError> {
    let limit: RateLimit =
        serde_json::from_str(json).map_err(|e| GhError::Parse(format!("rate_limit: {e}")))?;
    let graphql = limit.resources.graphql;
    Ok((graphql.remaining == 0).then_some(graphql.reset))
}

#[cfg(test)]
mod tests {
    //! `rate_limit.json` is `gh api rate_limit` against github.com with gh
    //! 2.74.2, 2026-11-17, with requests remaining.

    use super::*;

    fn fixture() -> String {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rate_limit.json");
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn only_a_spent_graphql_limit_reports_its_reset() {
        let recorded = fixture();
        assert_eq!(
            parse_graphql_reset(&recorded).unwrap(),
            None,
            "requests remain"
        );

        let mut spent: serde_json::Value = serde_json::from_str(&recorded).unwrap();
        let reset = spent["resources"]["graphql"]["reset"].as_i64().unwrap();
        spent["resources"]["graphql"]["remaining"] = 0.into();
        assert_eq!(
            parse_graphql_reset(&spent.to_string()).unwrap(),
            Some(reset)
        );
        // Another resource being spent says nothing about GraphQL requests.
        let mut core_spent: serde_json::Value = serde_json::from_str(&recorded).unwrap();
        core_spent["resources"]["core"]["remaining"] = 0.into();
        assert_eq!(parse_graphql_reset(&core_spent.to_string()).unwrap(), None);

        assert!(matches!(
            parse_graphql_reset("{\"message\":\"Not Found\"}"),
            Err(GhError::Parse(_))
        ));
    }
}
