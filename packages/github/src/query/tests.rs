//! Fixtures are real `gh api graphql` answers to `prs_for_branches_query`
//! (gh 2.74.2, 2026-11-12). `prs_for_branches.json` is `cli/cli` for three
//! branches; `prs_status_contexts.json` wraps a real Prow (`StatusContext`)
//! rollup from `kubernetes/kubernetes` in the same answer shape.
//! `prs_push_and_pull_request.json` is `serde-rs/serde` #3113, a same-repo
//! PR whose workflow runs on both `push` and `pull_request`.
//! `prs_number_not_found.json` is hand-written, not recorded: GitHub's
//! partial answer for a `pullRequest(number:)` that resolves to nothing.

use crate::query::{only_numbers_not_found, parse_prs_for_branches, prs_for_branches_query};
use crate::{CheckRollup, GhError, PrState, ReviewDecision};

fn fixture(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read_to_string(path).unwrap()
}

fn branches(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| (*n).to_owned()).collect()
}

#[test]
fn a_real_answer_parses_per_branch() {
    let names = branches(&[
        "williammartin-multiaccount-draft-08-evaluation",
        "trunk",
        "no-such-branch-xyz",
    ]);
    let answer = parse_prs_for_branches(&fixture("prs_for_branches.json"), &names, &[]).unwrap();
    assert_eq!(answer.default_branch.as_deref(), Some("trunk"));

    let draft = &answer.by_branch[&names[0]];
    assert_eq!(draft.len(), 1);
    let pr = &draft[0];
    assert_eq!(pr.number, 14927);
    assert_eq!(pr.state, PrState::Draft);
    assert!(!pr.cross_repo);
    assert_eq!(pr.review, Some(ReviewDecision::ReviewRequired));
    assert_eq!(pr.checks, CheckRollup::Failing { failed: Some(1) });
    assert_eq!(pr.failed[0].name, "build (windows-latest)");
    assert_eq!(
        pr.failed[0].workflow.as_deref(),
        Some("Unit and Integration Tests"),
        "the workflow name the query reads"
    );
    let job = pr.failed[0].job.expect("an Actions job");
    assert_eq!((job.run, job.job), (41_720_844_033, 126_212_878_432));

    let forks = &answer.by_branch[&names[1]];
    assert!(
        forks
            .iter()
            .all(|p| p.cross_repo && p.state == PrState::Closed)
    );
    assert!(answer.by_branch[&names[2]].is_empty());
}

#[test]
fn a_job_run_by_two_events_counts_as_two_checks() {
    let names = branches(&["deprecatedsince"]);
    let answer =
        parse_prs_for_branches(&fixture("prs_push_and_pull_request.json"), &names, &[]).unwrap();
    let pr = &answer.by_branch[&names[0]][0];
    assert_eq!((pr.number, pr.state), (3113, PrState::Merged));
    // 28 runs: 14 jobs, each once for `push` and once for `pull_request`,
    // all passing; neither event's run may stand in for the other's.
    assert_eq!(pr.checks, CheckRollup::Passing);
    assert!(pr.failed.is_empty());
}

#[test]
fn status_contexts_count_like_check_runs() {
    let names = branches(&["fix-admin-access-capacity-request"]);
    let answer = parse_prs_for_branches(&fixture("prs_status_contexts.json"), &names, &[]).unwrap();
    let pr = &answer.by_branch[&names[0]][0];
    assert_eq!(pr.checks, CheckRollup::Failing { failed: Some(1) });
    assert_eq!(pr.failed[0].name, "pull-kubernetes-linter-hints");
    assert_eq!(pr.failed[0].job, None, "Prow has no Actions job");
}

#[test]
fn an_error_answer_is_a_parse_error_carrying_the_message() {
    let json = r#"{"data":{"repository":null},"errors":[{"message":"Could not resolve to a Repository"}]}"#;
    let error = parse_prs_for_branches(json, &branches(&["a"]), &[]).unwrap_err();
    assert!(
        matches!(&error, GhError::Parse(m) if m.contains("Could not resolve")),
        "{error}"
    );
}

#[test]
fn a_truncated_check_list_defers_to_githubs_state() {
    let json = r#"{"data":{"repository":{"defaultBranchRef":{"name":"main"},"b0":{"nodes":[{
        "number":1,"url":"u","title":"t","state":"OPEN","isDraft":false,"headRefName":"a",
        "isCrossRepository":false,"reviewDecision":null,"updatedAt":"2026-01-01T00:00:00Z",
        "statusCheckRollup":{"state":"PENDING","contexts":{"pageInfo":{"hasNextPage":true},
        "nodes":[{"__typename":"CheckRun","name":"x","status":"COMPLETED","conclusion":"SUCCESS",
        "detailsUrl":null,"startedAt":null}]}}}]}}}}"#;
    let answer = parse_prs_for_branches(json, &branches(&["a"]), &[]).unwrap();
    assert_eq!(answer.by_branch["a"][0].checks, CheckRollup::Pending);
}

#[test]
fn a_truncated_failing_list_has_no_count_but_keeps_the_failures_in_hand() {
    let json = r#"{"data":{"repository":{"defaultBranchRef":{"name":"main"},"b0":{"nodes":[{
        "number":1,"url":"u","title":"t","state":"OPEN","isDraft":false,"headRefName":"a",
        "isCrossRepository":false,"reviewDecision":null,"updatedAt":"2026-01-01T00:00:00Z",
        "statusCheckRollup":{"state":"FAILURE","contexts":{"pageInfo":{"hasNextPage":true},
        "nodes":[{"__typename":"CheckRun","name":"x","status":"COMPLETED","conclusion":"FAILURE",
        "detailsUrl":null,"startedAt":null}]}}}]}}}}"#;
    let answer = parse_prs_for_branches(json, &branches(&["a"]), &[]).unwrap();
    let pr = &answer.by_branch["a"][0];
    assert_eq!(pr.checks, CheckRollup::Failing { failed: None });
    assert_eq!(pr.failed.len(), 1);
}

#[test]
fn a_repository_without_commits_has_no_default_branch() {
    let json = r#"{"data":{"repository":{"defaultBranchRef":null,"b0":{"nodes":[]}}}}"#;
    let answer = parse_prs_for_branches(json, &branches(&["a"]), &[]).unwrap();
    assert_eq!(answer.default_branch, None);
}

#[test]
fn the_query_names_every_field_the_parser_reads() {
    let query = prs_for_branches_query(1, 0);
    for field in [
        "defaultBranchRef",
        "isCrossRepository",
        "updatedAt",
        "hasNextPage",
        "... on CheckRun",
        "... on StatusContext",
    ] {
        assert!(query.contains(field), "{field}");
    }
}

#[test]
fn the_query_binds_one_variable_per_branch() {
    let query = prs_for_branches_query(2, 0);
    assert!(query.contains("$b0: String!, $b1: String!"));
    assert!(query.contains("b1: pullRequests(headRefName: $b1"));
    assert!(!query.contains("$b2"));
}

#[test]
fn the_query_binds_one_int_variable_per_number() {
    let query = prs_for_branches_query(1, 2);
    assert!(query.contains("$n0: Int!, $n1: Int!, $b0: String!"));
    assert!(query.contains("n1: pullRequest(number: $n1) { ...pr }"));
    assert!(!query.contains("$n2"));
}

/// Hand-written in the shape `pullRequest(number:)` answers: the `pr`
/// fragment's node, or null for a number the repository lacks.
#[test]
fn a_number_answers_its_pull_request_or_nothing() {
    let json = r#"{"data":{"repository":{"defaultBranchRef":{"name":"main"},"n0":{
        "number":9,"url":"u","title":"t","state":"OPEN","isDraft":false,"headRefName":"main",
        "isCrossRepository":true,"reviewDecision":null,"updatedAt":"2026-01-01T00:00:00Z",
        "statusCheckRollup":null},"n1":null,"b0":{"nodes":[]}}}}"#;
    let answer = parse_prs_for_branches(json, &branches(&["a"]), &[9, 404]).unwrap();
    let pr = answer.by_number[&9].as_ref().expect("PR 9");
    assert!(pr.cross_repo);
    assert_eq!(pr.head_branch, "main");
    assert_eq!(answer.by_number[&404], None);
    assert!(answer.by_branch["a"].is_empty());
}

#[test]
fn a_number_github_cannot_resolve_leaves_the_rest_of_the_answer() {
    let json = fixture("prs_number_not_found.json");
    assert!(only_numbers_not_found(&json));
    let answer = parse_prs_for_branches(&json, &branches(&["a"]), &[9, 404]).unwrap();
    assert_eq!(answer.by_number[&9].as_ref().map(|pr| pr.number), Some(9));
    assert_eq!(answer.by_number[&404], None);
    assert!(answer.by_branch["a"].is_empty());
}

#[test]
fn any_other_error_fails_the_answer() {
    let data =
        r#""data":{"repository":{"defaultBranchRef":{"name":"main"},"n0":null,"b0":{"nodes":[]}}}"#;
    // (label, errors)
    let cases = [
        (
            "the repository itself is not found",
            r#"[{"type":"NOT_FOUND","path":["repository"],"message":"no repo"}]"#,
        ),
        (
            "not found on a branch alias",
            r#"[{"type":"NOT_FOUND","path":["repository","b0"],"message":"x"}]"#,
        ),
        (
            "another kind of error on a number",
            r#"[{"type":"FORBIDDEN","path":["repository","n0"],"message":"x"}]"#,
        ),
        (
            "not found beside another error",
            r#"[{"type":"NOT_FOUND","path":["repository","n0"],"message":"x"},
                {"type":"RATE_LIMITED","message":"slow down"}]"#,
        ),
        (
            "an error with no type",
            r#"[{"message":"Something went wrong"}]"#,
        ),
    ];
    for (label, errors) in cases {
        let json = format!("{{{data},\"errors\":{errors}}}");
        assert!(!only_numbers_not_found(&json), "{label}");
        assert!(
            matches!(
                parse_prs_for_branches(&json, &branches(&["a"]), &[404]),
                Err(GhError::Parse(_))
            ),
            "{label}"
        );
    }
    let clean = format!("{{{data}}}");
    assert!(
        !only_numbers_not_found(&clean),
        "no errors is not a failure to excuse"
    );
}
