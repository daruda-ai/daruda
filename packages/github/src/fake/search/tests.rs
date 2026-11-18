use super::*;
use crate::fake::{GhFailure, GhOp};
use crate::{Gh, PrAudience, PrSearchContinuation, PrSearchCriteria, PrSearchState, RepoId};

#[test]
fn scripted_pages_keep_the_full_request_identity_and_failure_boundary() {
    let gh = FakeGh::new();
    let request = PrSearchRequest::new(
        "github.com",
        &[RepoId::new("github.com", "cli", "cli")],
        PrSearchCriteria {
            audience: PrAudience::All,
            state: PrSearchState::All,
            ..Default::default()
        },
        2,
    )
    .unwrap();
    gh.set_search_page(
        &request,
        include_str!("../../../tests/fixtures/search_page_first.json"),
    );
    let first = gh.search_prs_page(&request).unwrap();
    let PrSearchContinuation::More(next) = first.continuation else {
        panic!("next page");
    };
    gh.set_search_page(
        &next,
        include_str!("../../../tests/fixtures/search_page_next.json"),
    );
    let second = gh.clone().search_prs_page(&next).unwrap();
    assert_eq!((first.items.len(), second.items.len()), (2, 2));
    assert_ne!(first.items[0].pr.number, second.items[0].pr.number);
    assert_eq!(
        gh.lock().search_pages.len(),
        2,
        "clones share scoped scripts"
    );
    assert_eq!(gh.count(GhOp::SearchPrsPage), 2);
    assert_eq!(
        gh.calls()[1],
        GhCall::SearchPrsPage {
            request: next.clone()
        }
    );

    let mine = PrSearchRequest::new(
        request.host(),
        request.repos(),
        PrSearchCriteria::default(),
        2,
    )
    .unwrap();
    assert!(
        gh.search_prs_page(&mine).unwrap().items.is_empty(),
        "another audience cannot reuse the answer"
    );
    gh.fail(GhOp::SearchPrsPage, GhFailure::Parse("offline".into()));
    assert!(gh.search_prs_page(&request).is_err());
    gh.clear_failure(GhOp::SearchPrsPage);
    assert_eq!(gh.search_prs_page(&request).unwrap().items, first.items);
}

#[test]
fn a_fifty_first_match_is_reachable_and_refresh_starts_at_page_one() {
    let gh = FakeGh::new();
    let request = PrSearchRequest::new(
        "github.com",
        &[RepoId::new("github.com", "cli", "cli")],
        PrSearchCriteria {
            audience: PrAudience::All,
            ..Default::default()
        },
        50,
    )
    .unwrap();
    let mut answer: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/search_page_first.json"
    ))
    .unwrap();
    let node = answer["data"]["search"]["nodes"][0].clone();
    let nodes: Vec<_> = (1..=51)
        .map(|number| {
            let mut node = node.clone();
            node["number"] = number.into();
            node
        })
        .collect();
    let search = &mut answer["data"]["search"];
    search["issueCount"] = 51.into();
    search["nodes"] = nodes[..50].to_vec().into();
    search["pageInfo"]["endCursor"] = "fifty".into();
    gh.set_search_page(&request, &answer.to_string());
    let first = gh.search_prs_page(&request).unwrap();
    let PrSearchContinuation::More(next) = first.continuation else {
        panic!("next page");
    };
    let search = &mut answer["data"]["search"];
    search["nodes"] = nodes[50..].to_vec().into();
    search["pageInfo"]["hasNextPage"] = false.into();
    search["pageInfo"]["endCursor"] = "last".into();
    gh.set_search_page(&next, &answer.to_string());
    let second = gh.search_prs_page(&next).unwrap();
    assert_eq!(second.items[0].pr.number, 51);
    assert_eq!(second.continuation, PrSearchContinuation::Complete);
    assert_eq!(first.items.len() + second.items.len(), 51);
    assert_eq!(
        gh.search_prs_page(&request).unwrap().items.len(),
        50,
        "refresh has no inherited cursor"
    );
}

#[test]
fn listed_pages_narrow_by_state_and_scope_and_page_through_the_list() {
    use crate::{CheckRollup, PrAudience, PrListItem, PrState, PrSummary};

    let repo = RepoId::new("github.com", "o", "r");
    let other = RepoId::new("github.com", "o", "other");
    let item = |repo: &RepoId, number: u64, state: PrState| PrListItem {
        repo: repo.clone(),
        pr: PrSummary {
            number,
            url: String::new(),
            title: String::new(),
            state,
            head_branch: format!("h{number}"),
            cross_repo: false,
            checks: CheckRollup::None,
            failed: Vec::new(),
            review: None,
            updated_at: format!("2026-01-01T00:{:02}:{:02}Z", number / 60, number % 60),
        },
        author: None,
        head_owner: None,
        base_branch: "main".into(),
        comments: 0,
    };
    let gh = FakeGh::new();
    let mut items: Vec<PrListItem> = (1..=55).map(|n| item(&repo, n, PrState::Open)).collect();
    items.push(item(&repo, 90, PrState::Merged));
    items.push(item(&other, 91, PrState::Open));
    gh.set_pr_list(PrAudience::All, items);
    let request = |state| {
        PrSearchRequest::new(
            "github.com",
            std::slice::from_ref(&repo),
            PrSearchCriteria {
                audience: PrAudience::All,
                state,
                ..Default::default()
            },
            50,
        )
        .unwrap()
    };
    let first = gh.search_prs_page(&request(PrSearchState::Open)).unwrap();
    assert_eq!(first.items.len(), 50);
    assert_eq!(first.total_count, 55);
    assert_eq!(first.items[0].pr.number, 55, "newest first");
    let PrSearchContinuation::More(next) = first.continuation else {
        panic!("a second page");
    };
    let second = gh.search_prs_page(&next).unwrap();
    let numbers: Vec<u64> = second.items.iter().map(|i| i.pr.number).collect();
    assert_eq!(numbers, [5, 4, 3, 2, 1]);
    assert_eq!(second.continuation, PrSearchContinuation::Complete);

    let merged = gh.search_prs_page(&request(PrSearchState::Merged)).unwrap();
    let numbers: Vec<u64> = merged.items.iter().map(|i| i.pr.number).collect();
    assert_eq!(
        numbers,
        [90],
        "state narrows, scope excludes the other repository"
    );

    gh.set_search_truncated(PrAudience::All, true);
    let merged = gh.search_prs_page(&request(PrSearchState::Merged)).unwrap();
    assert_eq!(merged.continuation, PrSearchContinuation::Incomplete);
}

#[test]
fn list_items_sort_most_recently_updated_first() {
    use crate::{CheckRollup, PrListItem, PrState, PrSummary};

    let item = |number: u64, updated_at: &str| PrListItem {
        repo: RepoId::new("github.com", "o", "r"),
        pr: PrSummary {
            number,
            url: String::new(),
            title: String::new(),
            state: PrState::Open,
            head_branch: "h".into(),
            cross_repo: false,
            checks: CheckRollup::None,
            failed: Vec::new(),
            review: None,
            updated_at: updated_at.to_owned(),
        },
        author: None,
        head_owner: None,
        base_branch: "main".into(),
        comments: 0,
    };
    let mut items = vec![
        item(1, "2026-11-02T00:00:00Z"),
        item(2, "2026-11-01T00:00:00Z"),
        item(3, "2026-11-03T00:00:00Z"),
        item(4, "2026-11-02T00:00:00Z"),
    ];
    super::newest_first(&mut items);
    let numbers: Vec<u64> = items.iter().map(|i| i.pr.number).collect();
    assert_eq!(numbers, [3, 1, 4, 2], "ties keep their order");
}
