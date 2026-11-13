use std::path::{Path, PathBuf};

use super::*;

// sha256("abc")
const ABC: Asset = Asset {
    name: "fake-abc",
    sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
};

#[test]
fn target_selects_the_release_asset_per_host() {
    let rows: &[(&str, &str, &str, bool, Option<&str>)] = &[
        (
            "macos arm",
            "macos",
            "aarch64",
            false,
            Some("agent-browser-darwin-arm64"),
        ),
        (
            "macos intel",
            "macos",
            "x86_64",
            false,
            Some("agent-browser-darwin-x64"),
        ),
        (
            "linux x64 glibc",
            "linux",
            "x86_64",
            false,
            Some("agent-browser-linux-x64"),
        ),
        (
            "linux x64 musl",
            "linux",
            "x86_64",
            true,
            Some("agent-browser-linux-musl-x64"),
        ),
        (
            "linux arm glibc",
            "linux",
            "aarch64",
            false,
            Some("agent-browser-linux-arm64"),
        ),
        (
            "linux arm musl",
            "linux",
            "aarch64",
            true,
            Some("agent-browser-linux-musl-arm64"),
        ),
        (
            "windows x64",
            "windows",
            "x86_64",
            false,
            Some("agent-browser-win32-x64.exe"),
        ),
        ("windows arm", "windows", "aarch64", false, None),
        ("freebsd", "freebsd", "x86_64", false, None),
    ];
    for (label, os, arch, musl, expected) in rows {
        let asset = target(os, arch, *musl);
        assert_eq!(asset.map(|asset| asset.name), *expected, "{label}");
        if let Some(asset) = asset {
            assert_eq!(asset.sha256.len(), 64, "{label}: digest length");
            assert!(
                asset.sha256.bytes().all(|b| b.is_ascii_hexdigit()),
                "{label}: digest is hex"
            );
        }
    }
}

#[test]
fn download_url_names_the_pinned_release() {
    assert_eq!(
        download_url(&pinned::LINUX_X64),
        "https://github.com/vercel-labs/agent-browser/releases/download/v0.43.0/agent-browser-linux-x64"
    );
}

#[test]
fn verify_compares_the_sha256_digest() {
    let upper = Asset {
        sha256: "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD",
        ..ABC
    };
    let rows: &[(&str, &[u8], Asset, bool)] = &[
        ("matching digest", b"abc", ABC, true),
        ("uppercase pinned digest", b"abc", upper, true),
        ("different bytes", b"abd", ABC, false),
        ("empty bytes", b"", ABC, false),
    ];
    for (label, bytes, asset, ok) in rows {
        let result = verify(bytes, asset);
        assert_eq!(result.is_ok(), *ok, "{label}: {result:?}");
        if let Err(error) = result {
            assert!(
                matches!(
                    error,
                    BrowserToolError::ChecksumMismatch {
                        asset: "fake-abc",
                        ..
                    }
                ),
                "{label}: {error:?}"
            );
        }
    }
}

#[test]
fn binary_file_name_adds_exe_on_windows() {
    assert_eq!(binary_file_name("windows"), "agent-browser.exe");
    assert_eq!(binary_file_name("macos"), "agent-browser");
    assert_eq!(binary_file_name("linux"), "agent-browser");
}

#[test]
fn session_name_is_a_short_stable_hash_of_the_lane_root() {
    let temp = tempfile::tempdir().unwrap();
    let lane = temp.path().join("lane");
    let other = temp.path().join("other");
    std::fs::create_dir(&lane).unwrap();
    std::fs::create_dir(&other).unwrap();

    let name = session_name(&lane);
    assert!(name.starts_with("daruda-"), "{name}");
    assert_eq!(name.len(), 19, "{name}");
    assert!(
        name.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "{name}"
    );
    assert_eq!(session_name(&lane), name, "deterministic");
    assert_ne!(session_name(&other), name, "distinct lanes");
    assert_eq!(session_name(&lane.join(".")), name, "trailing dot");
    assert_eq!(
        session_name(&other.join("..").join("lane")),
        name,
        "parent traversal"
    );
    let missing = temp.path().join("missing");
    assert_eq!(
        session_name(&missing),
        session_name(&missing),
        "uncanonicalizable root still hashes"
    );
}

#[cfg(unix)]
#[test]
fn session_name_follows_a_symlinked_lane_root() {
    let temp = tempfile::tempdir().unwrap();
    let lane = temp.path().join("lane");
    std::fs::create_dir(&lane).unwrap();
    let link = temp.path().join("link");
    std::os::unix::fs::symlink(&lane, &link).unwrap();
    assert_eq!(session_name(&link), session_name(&lane));
}

#[test]
fn launch_runs_mcp_with_the_lane_session_and_socket_dir() {
    let temp = tempfile::tempdir().unwrap();
    let binary = Path::new("/opt/agent-browser");
    let sockets = Path::new("/tmp/ab");
    let launch = launch(binary, sockets, temp.path(), None).unwrap();
    assert_eq!(launch.command, PathBuf::from("/opt/agent-browser"));
    assert_eq!(launch.args, vec!["mcp".to_owned()]);
    assert_eq!(
        launch.env,
        vec![
            (
                "AGENT_BROWSER_SESSION".to_owned(),
                session_name(temp.path())
            ),
            ("AGENT_BROWSER_SOCKET_DIR".to_owned(), "/tmp/ab".to_owned()),
        ]
    );
}

#[test]
fn launch_enforces_only_a_given_socket_path_limit() {
    let lane = Path::new("/nonexistent/lane");
    // `<dir>/daruda-<12 hex>.sock` adds 25 bytes to the directory.
    let fits = PathBuf::from(format!("/{}", "s".repeat(UNIX_SOCKET_PATH_LIMIT - 26)));
    let long = PathBuf::from(format!("/{}", "s".repeat(UNIX_SOCKET_PATH_LIMIT)));
    let binary = Path::new("agent-browser");
    let rows: &[(&str, &Path, Option<usize>, Option<usize>)] = &[
        (
            "exactly at the limit",
            &fits,
            Some(UNIX_SOCKET_PATH_LIMIT),
            None,
        ),
        (
            "over the limit",
            &long,
            Some(UNIX_SOCKET_PATH_LIMIT),
            Some(UNIX_SOCKET_PATH_LIMIT + 1 + 25),
        ),
        ("no limit", &long, None, None),
    ];
    for (label, dir, limit, too_long) in rows {
        let result = launch(binary, dir, lane, *limit);
        match (result, too_long) {
            (Ok(_), None) => {}
            (Err(BrowserToolError::SocketPathTooLong { len, limit }), Some(expected)) => {
                assert_eq!(len, *expected, "{label}");
                assert_eq!(limit, UNIX_SOCKET_PATH_LIMIT, "{label}");
            }
            (other, _) => panic!("{label}: {other:?}"),
        }
    }
}
