use daruda_github::model::is_openable_link;
use daruda_github::{CheckRollup, PrState, PrSummary};

fn pr(url: &str) -> PrSummary {
    PrSummary {
        number: 1,
        url: url.into(),
        title: String::new(),
        state: PrState::Open,
        head_branch: "feat".into(),
        cross_repo: false,
        checks: CheckRollup::None,
        failed: Vec::new(),
        review: None,
        updated_at: String::new(),
    }
}

#[test]
fn only_a_web_page_is_offered_to_the_browser() {
    let cases = [
        ("https://github.com/o/r/pull/1", true),
        ("http://ghe.corp/o/r/pull/1", true),
        ("file:///etc/passwd", false),
        ("javascript:alert(1)", false),
        ("ssh://git@github.com/o/r", false),
        ("", false),
    ];
    for (url, expected) in cases {
        assert_eq!(pr(url).web_url().is_some(), expected, "{url}");
    }
}

#[test]
fn a_link_in_github_text_opens_only_a_web_page_or_an_address() {
    let cases = [
        ("https://example.com/a", true),
        ("http://example.com/a", true),
        ("mailto:a@example.com", true),
        ("file:///etc/passwd", false),
        ("javascript:alert(1)", false),
        ("vscode://file/etc/passwd", false),
        ("x-apple.systempreferences:com.apple.preference", false),
        ("smb://host/share", false),
        ("/o/r/issues/1", false),
        ("#heading", false),
        ("", false),
    ];
    for (url, expected) in cases {
        assert_eq!(is_openable_link(url), expected, "{url}");
    }
}

#[test]
fn only_a_login_or_a_team_slug_is_a_reviewer_name() {
    use daruda_github::reviewer_name_ok;
    // (label, name, ok)
    let cases = [
        ("a login", "octo-cat", true),
        ("a managed enterprise login", "Mona_corp", true),
        ("a team starting with a dot", "org/.x", false),
        ("a team", "my-org/core.team_1", true),
        ("a flag", "--admin", false),
        ("a leading hyphen", "-x", false),
        ("a space", "a b", false),
        ("a comma", "a,b", false),
        ("an empty team", "org/", false),
        ("a nested path", "org/team/x", false),
        ("empty", "", false),
    ];
    for (label, name, ok) in cases {
        assert_eq!(reviewer_name_ok(name), ok, "{label}");
    }
}
