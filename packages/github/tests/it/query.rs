//! Fixtures are real `gh api graphql` answers to `prs_for_branches_query`
//! (gh 2.74.2, 2026-11-12). `prs_for_branches.json` is `cli/cli` for three
//! branches; `prs_status_contexts.json` wraps a real Prow (`StatusContext`)
//! rollup from `kubernetes/kubernetes` in the same answer shape.

use daruda_github::query::{parse_prs_for_branches, prs_for_branches_query};
use daruda_github::{CheckRollup, GhError, PrState, ReviewDecision};

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
    let answer = parse_prs_for_branches(&fixture("prs_for_branches.json"), &names).unwrap();
    assert_eq!(answer.default_branch.as_deref(), Some("trunk"));

    let draft = &answer.by_branch[&names[0]];
    assert_eq!(draft.len(), 1);
    let pr = &draft[0];
    assert_eq!(pr.number, 14927);
    assert_eq!(pr.state, PrState::Draft);
    assert!(!pr.cross_repo);
    assert_eq!(pr.review, Some(ReviewDecision::ReviewRequired));
    assert_eq!(pr.checks, CheckRollup::Failing { failed: 1 });
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
fn status_contexts_count_like_check_runs() {
    let names = branches(&["fix-admin-access-capacity-request"]);
    let answer = parse_prs_for_branches(&fixture("prs_status_contexts.json"), &names).unwrap();
    let pr = &answer.by_branch[&names[0]][0];
    assert_eq!(pr.checks, CheckRollup::Failing { failed: 1 });
    assert_eq!(pr.failed[0].name, "pull-kubernetes-linter-hints");
    assert_eq!(pr.failed[0].job, None, "Prow has no Actions job");
}

#[test]
fn an_error_answer_is_a_parse_error_carrying_the_message() {
    let json = r#"{"data":{"repository":null},"errors":[{"message":"Could not resolve to a Repository"}]}"#;
    let error = parse_prs_for_branches(json, &branches(&["a"])).unwrap_err();
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
    let answer = parse_prs_for_branches(json, &branches(&["a"])).unwrap();
    assert_eq!(answer.by_branch["a"][0].checks, CheckRollup::Pending);
}

#[test]
fn a_repository_without_commits_has_no_default_branch() {
    let json = r#"{"data":{"repository":{"defaultBranchRef":null,"b0":{"nodes":[]}}}}"#;
    let answer = parse_prs_for_branches(json, &branches(&["a"])).unwrap();
    assert_eq!(answer.default_branch, None);
}

#[test]
fn the_query_names_every_field_the_parser_reads() {
    let query = prs_for_branches_query(1);
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
