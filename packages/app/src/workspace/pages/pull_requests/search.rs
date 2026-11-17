//! Local narrowing never spends a GitHub request or hides host failures.

use super::list::{GroupBody, PageBody, PrPageView};

/// Keep the rows whose fields hold `query` (already trimmed), counting
/// `(visible, total)`.
pub(super) fn narrow(list: &mut PrPageView, query: &str) -> (usize, usize) {
    let PageBody::Groups(groups) = &mut list.body else {
        return (0, 0);
    };
    let query = query.to_lowercase();
    let mut total = 0;
    let mut visible = 0;
    for group in groups {
        if let GroupBody::Listed { rows, .. } = &mut group.body {
            total += rows.len();
            rows.retain(|row| row.haystack.contains(&query));
            visible += rows.len();
            group.meta =
                crate::surface::strings::pull_requests::group_count(group.repo.host(), rows.len());
        }
    }
    (visible, total)
}

#[cfg(test)]
mod tests {
    use daruda_github::{CheckRollup, PrListItem, PrState, PrSummary, RepoId};
    use daruda_store::project::LaneRef;

    use super::super::list::{LaneFacts, haystack};

    #[test]
    fn search_finds_a_pull_request_by_its_fields_in_any_case() {
        let item = PrListItem {
            repo: RepoId::new("github.com", "daruda", "app"),
            pr: PrSummary {
                number: 42,
                url: String::new(),
                title: "Fix navigation".into(),
                state: PrState::Open,
                head_branch: "feat/scope".into(),
                cross_repo: false,
                checks: CheckRollup::None,
                failed: Vec::new(),
                review: None,
                updated_at: "2026-01-01T00:00:00Z".into(),
            },
            author: Some("Woo".into()),
            head_owner: None,
            base_branch: "main".into(),
            comments: 0,
        };
        let lane = LaneFacts {
            lane: LaneRef {
                project: 1,
                lane: 2,
            },
            name: "Scope work".into(),
            branch: None,
            pr: None,
        };
        let found = haystack(&item, Some(&lane));
        // (query, found)
        for (query, expected) in [
            ("", true),
            ("#42", true),
            ("navigation", true),
            ("feat/scope", true),
            ("main", true),
            ("woo", true),
            ("daruda/app", true),
            ("scope work", true),
            ("ago", false),
            ("missing", false),
        ] {
            assert_eq!(found.contains(query), expected, "{query}");
        }
    }
}
