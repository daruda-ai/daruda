use std::cell::{Cell, RefCell};

use super::*;

const OS: &str = "linux";
const BYTES: &[u8] = b"abc";
// sha256("abc")
const ASSET: Asset = Asset {
    name: "fake-abc",
    sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
    size: 3,
};

struct FakeFetch {
    bytes: Vec<u8>,
    calls: Cell<usize>,
}

impl FakeFetch {
    fn new(bytes: &[u8]) -> Self {
        Self {
            bytes: bytes.to_vec(),
            calls: Cell::new(0),
        }
    }
}

impl Fetch for FakeFetch {
    fn get(&self, _: &str) -> Result<Vec<u8>, InstallError> {
        self.calls.set(self.calls.get() + 1);
        Ok(self.bytes.clone())
    }
}

struct FakeRun {
    succeed: Cell<bool>,
    calls: Cell<usize>,
}

impl FakeRun {
    fn new(succeed: bool) -> Self {
        Self {
            succeed: Cell::new(succeed),
            calls: Cell::new(0),
        }
    }
}

impl Run for FakeRun {
    fn run_install(&self, _: &Path) -> Result<(), InstallError> {
        self.calls.set(self.calls.get() + 1);
        if self.succeed.get() {
            Ok(())
        } else {
            Err(InstallError::BrowserRun(CaptureError::Spawn(
                std::io::Error::other("no browser"),
            )))
        }
    }
}

fn run_install(
    tool_dir: &Path,
    fetch: &FakeFetch,
    run: &FakeRun,
) -> (Result<PathBuf, InstallError>, Vec<InstallStage>) {
    let stages = RefCell::new(Vec::new());
    let result = ToolInstall::new(tool_dir, ASSET, OS).install(
        &mut |stage| stages.borrow_mut().push(stage),
        fetch,
        run,
    );
    (result, stages.into_inner())
}

fn cached(tool_dir: &Path) -> Option<PathBuf> {
    ToolInstall::new(tool_dir, ASSET, OS).cached()
}

fn binary_path(tool_dir: &Path) -> PathBuf {
    tool_dir.join(VERSION).join(binary_file_name(OS))
}

fn marker_path(tool_dir: &Path) -> PathBuf {
    tool_dir.join(VERSION).join(MARKER)
}

#[test]
fn checksum_mismatch_publishes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let fetch = FakeFetch::new(b"not abc");
    let run = FakeRun::new(true);

    let (result, _) = run_install(dir.path(), &fetch, &run);

    assert!(matches!(result, Err(InstallError::Verify(_))), "{result:?}");
    assert!(!binary_path(dir.path()).exists());
    assert!(!marker_path(dir.path()).exists());
    assert_eq!(run.calls.get(), 0);
}

#[test]
fn success_publishes_binary_and_marker_then_short_circuits() {
    let dir = tempfile::tempdir().unwrap();
    let fetch = FakeFetch::new(BYTES);
    let run = FakeRun::new(true);

    let (result, stages) = run_install(dir.path(), &fetch, &run);

    let binary = result.unwrap();
    assert_eq!(binary, binary_path(dir.path()));
    assert_eq!(std::fs::read(&binary).unwrap(), BYTES);
    assert!(marker_path(dir.path()).is_file());
    assert_eq!(cached(dir.path()), Some(binary.clone()));
    assert_eq!(stages, [InstallStage::Binary, InstallStage::Browser]);

    let (again, stages) = run_install(dir.path(), &fetch, &run);
    assert_eq!(again.unwrap(), binary);
    assert!(stages.is_empty());
    assert_eq!(fetch.calls.get(), 1);
    assert_eq!(run.calls.get(), 1);
}

#[test]
fn browser_failure_keeps_the_binary_for_a_retry() {
    let dir = tempfile::tempdir().unwrap();
    let fetch = FakeFetch::new(BYTES);
    let run = FakeRun::new(false);

    let (result, _) = run_install(dir.path(), &fetch, &run);

    assert!(
        matches!(result, Err(InstallError::BrowserRun(_))),
        "{result:?}"
    );
    assert_eq!(std::fs::read(binary_path(dir.path())).unwrap(), BYTES);
    assert!(!marker_path(dir.path()).exists());
    assert_eq!(cached(dir.path()), None);

    run.succeed.set(true);
    let (retry, stages) = run_install(dir.path(), &fetch, &run);

    assert_eq!(retry.unwrap(), binary_path(dir.path()));
    assert_eq!(stages, [InstallStage::Binary, InstallStage::Browser]);
    assert_eq!(fetch.calls.get(), 1);
    assert_eq!(run.calls.get(), 2);
    assert!(marker_path(dir.path()).is_file());
}

#[test]
fn cached_needs_the_marker_and_the_pinned_size() {
    let rows: &[(&str, Option<&[u8]>, bool, bool)] = &[
        ("installed", Some(BYTES), true, true),
        ("same-size content is not hashed", Some(b"abd"), true, true),
        ("shorter binary", Some(b"ab"), true, false),
        ("longer binary", Some(b"abcd"), true, false),
        ("no marker", Some(BYTES), false, false),
        ("no binary", None, true, false),
    ];
    for (label, binary, marker, found) in rows {
        let dir = tempfile::tempdir().unwrap();
        let (result, _) = run_install(dir.path(), &FakeFetch::new(BYTES), &FakeRun::new(true));
        result.unwrap();
        match binary {
            Some(bytes) => std::fs::write(binary_path(dir.path()), bytes).unwrap(),
            None => std::fs::remove_file(binary_path(dir.path())).unwrap(),
        }
        if !marker {
            std::fs::remove_file(marker_path(dir.path())).unwrap();
        }

        let expected = found.then(|| binary_path(dir.path()));
        assert_eq!(cached(dir.path()), expected, "{label}");
    }
}

#[test]
fn install_replaces_a_same_size_tampered_binary() {
    let dir = tempfile::tempdir().unwrap();
    let fetch = FakeFetch::new(BYTES);
    let run = FakeRun::new(true);
    run_install(dir.path(), &fetch, &run).0.unwrap();
    std::fs::write(binary_path(dir.path()), b"abd").unwrap();

    let (result, stages) = run_install(dir.path(), &fetch, &run);

    assert_eq!(result.unwrap(), binary_path(dir.path()));
    assert_eq!(std::fs::read(binary_path(dir.path())).unwrap(), BYTES);
    assert_eq!(stages, [InstallStage::Binary, InstallStage::Browser]);
    assert_eq!(fetch.calls.get(), 2);
}

#[test]
fn read_capped_refuses_an_oversized_body() {
    // (label, body, cap, accepted whole)
    let rows: &[(&str, &[u8], u64, bool)] = &[
        ("under the cap", b"ab", 3, true),
        ("exactly the cap", b"abc", 3, true),
        ("one byte over", b"abcd", 3, false),
        ("empty", b"", 0, true),
    ];
    for (label, body, max, accepted) in rows {
        let result = read_capped(*body, *max);
        if *accepted {
            assert_eq!(result.unwrap(), *body, "{label}");
        } else {
            assert!(
                matches!(&result, Err(InstallError::TooLarge { max: m }) if m == max),
                "{label}: {result:?}"
            );
        }
    }
}

#[test]
fn tail_keeps_the_last_characters() {
    let rows: &[(&str, &str, usize, &str)] = &[
        ("shorter than the cap", "abc", 5, "abc"),
        ("cut to the cap", "abcdef", 3, "def"),
        ("multibyte boundary", "가나다라", 2, "다라"),
        ("zero cap", "abc", 0, ""),
    ];
    for (label, text, max, expected) in rows {
        assert_eq!(tail(text, *max), *expected, "{label}");
    }
}

#[test]
fn a_browser_install_past_its_deadline_fails() {
    let result =
        browser_install_outcome(Err(CaptureError::Timeout(BROWSER_INSTALL_LIMITS.timeout)));

    assert!(
        matches!(
            result,
            Err(InstallError::BrowserRun(CaptureError::Timeout(_)))
        ),
        "{result:?}"
    );
}

#[cfg(unix)]
#[test]
fn a_browser_install_reports_its_exit_and_stderr_tail() {
    use std::os::unix::process::ExitStatusExt as _;

    let captured = |code: i32, stderr: &str| Captured {
        status: std::process::ExitStatus::from_raw(code << 8),
        stdout: Vec::new(),
        stderr: stderr.as_bytes().to_vec(),
    };
    assert!(browser_install_outcome(Ok(captured(0, "noise"))).is_ok());

    let result = browser_install_outcome(Ok(captured(1, "chrome: no space left\n")));
    let Err(InstallError::BrowserExit {
        status,
        stderr_tail,
    }) = result
    else {
        panic!("{result:?}");
    };
    assert_eq!(status.code(), Some(1));
    assert_eq!(stderr_tail, "chrome: no space left");
}

#[cfg(unix)]
#[test]
fn published_binary_is_executable() {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = tempfile::tempdir().unwrap();
    let (result, _) = run_install(dir.path(), &FakeFetch::new(BYTES), &FakeRun::new(true));

    let mode = std::fs::metadata(result.unwrap())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o111, 0o111, "mode {mode:o}");
}
