use daruda_github::RepoId;
use daruda_github::remote_parse::parse_remote;

fn repo(host: &str, owner: &str, name: &str) -> Option<RepoId> {
    Some(RepoId {
        host: host.into(),
        owner: owner.into(),
        name: name.into(),
    })
}

#[test]
fn remote_urls_name_their_repository() {
    let cases = [
        (
            "https with .git",
            "https://github.com/O/R.git",
            repo("github.com", "o", "r"),
        ),
        (
            "https without .git",
            "https://github.com/o/r",
            repo("github.com", "o", "r"),
        ),
        (
            "trailing slash",
            "https://github.com/o/r/",
            repo("github.com", "o", "r"),
        ),
        (
            "credentials in https",
            "https://x:tok@github.com/o/r",
            repo("github.com", "o", "r"),
        ),
        (
            "scp-like",
            "git@github.com:o/r.git",
            repo("github.com", "o", "r"),
        ),
        (
            "ssh with port",
            "ssh://git@ghe.corp:2222/o/r",
            repo("ghe.corp", "o", "r"),
        ),
        (
            "ssh over 443",
            "ssh://git@ssh.github.com:443/o/r.git",
            repo("github.com", "o", "r"),
        ),
        (
            "host case",
            "https://GitHub.com/o/r",
            repo("github.com", "o", "r"),
        ),
        (
            "surrounding whitespace",
            " git@github.com:o/r.git\n",
            repo("github.com", "o", "r"),
        ),
        ("too deep", "https://gitlab.com/group/sub/r.git", None),
        ("too shallow", "https://github.com/o", None),
        ("absolute path", "/srv/git/r.git", None),
        ("relative path", "../r.git", None),
        ("file url", "file:///srv/git/r.git", None),
        ("windows path", "C:\\repos\\r.git", None),
        ("windows path with slashes", "C:/repos/r.git", None),
        ("empty", "", None),
    ];
    for (label, url, expected) in cases {
        assert_eq!(parse_remote(url), expected, "{label}: {url}");
    }
}

#[test]
fn qualified_names_include_the_host() {
    let id = parse_remote("git@ghe.corp:Team/App.git").unwrap();
    assert_eq!(id.qualified(), "ghe.corp/team/app");
}
