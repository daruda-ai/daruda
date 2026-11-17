//! Local narrowing never spends a GitHub request or hides host failures.

use super::list::{GroupBody, PageBody, PrPageView};

pub(super) fn narrow(list: &mut PrPageView, query: &str) -> (usize, usize) {
    let PageBody::Groups(groups) = &mut list.body else {
        return (0, 0);
    };
    let query = query.trim().to_lowercase();
    let mut total = 0;
    let mut visible = 0;
    for group in groups {
        if let GroupBody::Listed { rows, .. } = &mut group.body {
            total += rows.len();
            rows.retain(|row| matches(&query, &[&row.title, &row.meta, &group.label]));
            visible += rows.len();
            group.meta =
                crate::surface::strings::pull_requests::group_count(group.repo.host(), rows.len());
        }
    }
    (visible, total)
}

fn matches(query: &str, fields: &[&str]) -> bool {
    fields
        .iter()
        .any(|field| field.to_lowercase().contains(query))
}

#[cfg(test)]
mod tests {
    #[test]
    fn search_matches_number_branch_author_and_repo_without_case() {
        let fields = [
            "#42 Fix navigation",
            "feat/scope → main · Woo",
            "daruda/app",
        ];
        for (query, expected) in [
            ("", true),
            ("#42", true),
            ("scope", true),
            ("woo", true),
            ("daruda", true),
            ("missing", false),
        ] {
            assert_eq!(super::matches(query, &fields), expected, "{query}");
        }
    }
}
