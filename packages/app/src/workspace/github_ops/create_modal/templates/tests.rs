use super::*;

fn tree(files: &[(&str, &str)]) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for (path, body) in files {
        let path = root.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }
    root
}

#[test]
fn templates_are_found_where_github_looks() {
    let root = tree(&[
        (".github/PULL_REQUEST_TEMPLATE.md", "## Why"),
        ("docs/pull_request_template.md", "docs one"),
        (".github/PULL_REQUEST_TEMPLATE/bug.md", "## Bug"),
        (".github/PULL_REQUEST_TEMPLATE/feature.MD", "## Feature"),
        (".github/PULL_REQUEST_TEMPLATE/notes.txt", "not a template"),
        ("src/pull_request_template.md", "not where GitHub looks"),
    ]);
    let found = find_templates(root.path());
    // The first place in GitHub's order wins: docs/ before .github/.
    assert_eq!(
        found.default,
        Some(PrTemplate {
            path: "docs/pull_request_template.md".into(),
            body: "docs one".into(),
        })
    );
    let named: Vec<&str> = found.named.iter().map(|t| t.path.as_str()).collect();
    assert_eq!(
        named,
        [
            ".github/PULL_REQUEST_TEMPLATE/bug.md",
            ".github/PULL_REQUEST_TEMPLATE/feature.MD"
        ]
    );
    assert_eq!(found.all().count(), 3);
}

#[test]
fn none_or_an_oversized_file_offers_nothing() {
    let empty = tree(&[]);
    assert!(find_templates(empty.path()).is_empty());
    let big = "x".repeat(TEMPLATE_MAX_BYTES as usize + 1);
    let root = tree(&[("pull_request_template.md", &big)]);
    assert!(find_templates(root.path()).is_empty());
}

#[cfg(unix)]
#[test]
fn a_symlinked_template_is_never_followed() {
    let outside = tree(&[("secret", "do not read")]);
    let root = tree(&[]);
    std::fs::create_dir_all(root.path().join(".github/PULL_REQUEST_TEMPLATE")).unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("secret"),
        root.path().join(".github/pull_request_template.md"),
    )
    .unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("secret"),
        root.path().join(".github/PULL_REQUEST_TEMPLATE/linked.md"),
    )
    .unwrap();
    let linked_dir = root.path().join("docs");
    std::os::unix::fs::symlink(outside.path(), &linked_dir).unwrap();
    assert!(find_templates(root.path()).is_empty());
}
