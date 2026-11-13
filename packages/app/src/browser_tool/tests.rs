use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use gpui::{BorrowAppContext as _, TestAppContext};

use super::*;

/// Answers from memory; never downloads or runs anything.
struct FakeInstaller {
    cached: Option<PathBuf>,
    /// Each call takes the next answer; the last one repeats.
    answers: Mutex<Vec<Result<PathBuf, String>>>,
    calls: AtomicUsize,
}

impl FakeInstaller {
    fn new(cached: Option<PathBuf>, answers: Vec<Result<PathBuf, String>>) -> Arc<Self> {
        Arc::new(Self {
            cached,
            answers: Mutex::new(answers),
            calls: AtomicUsize::new(0),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Installer for FakeInstaller {
    fn cached(&self) -> Option<PathBuf> {
        self.cached.clone()
    }

    fn install(&self, on_stage: &mut dyn FnMut(InstallStage)) -> Result<PathBuf, InstallError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        on_stage(InstallStage::Binary);
        on_stage(InstallStage::Browser);
        let mut answers = self.answers.lock().expect("answers lock");
        let answer = if answers.len() > 1 {
            answers.remove(0)
        } else {
            answers[0].clone()
        };
        answer.map_err(InstallError::Browser)
    }
}

fn binary() -> PathBuf {
    PathBuf::from("/opt/agent-browser/agent-browser")
}

fn socket_dir() -> PathBuf {
    PathBuf::from("/tmp/ab")
}

fn install_tool(installer: Arc<FakeInstaller>, enabled: bool, cx: &mut TestAppContext) {
    cx.update(|cx| {
        SettingsStore::init(cx);
        set_enabled(enabled, cx);
        cx.set_global(BrowserTool::supported(installer, socket_dir()));
    });
}

fn set_enabled(enabled: bool, cx: &mut App) {
    let mut config = (*SettingsStore::global(cx).user_arc()).clone();
    config.browser.enabled = enabled;
    cx.update_global::<SettingsStore, _>(|store, _| store.set_user_for_testing(config));
}

fn toggle(enabled: bool, cx: &mut TestAppContext) {
    cx.update(|cx| {
        set_enabled(enabled, cx);
        BrowserTool::settings_changed(cx);
    });
    cx.run_until_parked();
}

fn state(cx: &mut TestAppContext) -> BrowserToolState {
    cx.update(|cx| match cx.global::<BrowserTool>() {
        BrowserTool::Supported(supported) => supported.state.clone(),
        BrowserTool::Unsupported { .. } => panic!("expected a supported host"),
    })
}

#[gpui::test]
fn startup_state_follows_the_cache(cx: &mut TestAppContext) {
    let cases = [
        ("cached", Some(binary()), BrowserToolState::Ready(binary())),
        ("not cached", None, BrowserToolState::Absent),
    ];
    for (label, cached, expected) in cases {
        install_tool(FakeInstaller::new(cached, vec![Ok(binary())]), false, cx);
        assert_eq!(state(cx), expected, "{label}");
    }
}

#[gpui::test]
fn the_host_target_decides_support(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().expect("tool dir");
    let asset = browser_tool::target("macos", "aarch64", false).expect("a release");
    let supported = host_tool(Some(asset), dir.path().to_path_buf(), socket_dir());
    assert!(
        matches!(&supported, BrowserTool::Supported(s) if s.state == BrowserToolState::Absent),
        "an empty tool directory has nothing cached"
    );
    let unsupported = host_tool(None, dir.path().to_path_buf(), socket_dir());
    assert!(matches!(
        unsupported,
        BrowserTool::Unsupported { notified: false }
    ));

    cx.update(|cx| {
        SettingsStore::init(cx);
        set_enabled(true, cx);
        cx.set_global(unsupported);
        BrowserTool::settings_changed(cx);
        assert!(matches!(
            cx.global::<BrowserTool>(),
            BrowserTool::Unsupported { notified: true }
        ));
    });
}

#[gpui::test]
fn enabling_installs_through_both_stages(cx: &mut TestAppContext) {
    let installer = FakeInstaller::new(None, vec![Ok(binary())]);
    install_tool(installer.clone(), false, cx);
    let seen = Rc::new(RefCell::new(Vec::new()));
    let _observer = cx.update(|cx| {
        let seen = seen.clone();
        cx.observe_global::<BrowserTool>(move |cx| {
            if let BrowserTool::Supported(supported) = cx.global::<BrowserTool>() {
                seen.borrow_mut().push(supported.state.clone());
            }
        })
    });
    cx.run_until_parked();

    toggle(true, cx);

    assert_eq!(state(cx), BrowserToolState::Ready(binary()));
    assert_eq!(installer.calls(), 1);
    let seen = seen.borrow();
    let browser = seen
        .iter()
        .position(|s| *s == BrowserToolState::Installing(InstallStage::Browser));
    let binary_stage = seen
        .iter()
        .position(|s| *s == BrowserToolState::Installing(InstallStage::Binary));
    assert!(
        binary_stage < browser && browser.is_some(),
        "stages in order: {seen:?}"
    );
    assert_eq!(seen.last(), Some(&BrowserToolState::Ready(binary())));
}

#[gpui::test]
fn a_second_change_while_installing_does_not_start_another(cx: &mut TestAppContext) {
    let installer = FakeInstaller::new(None, vec![Ok(binary())]);
    install_tool(installer.clone(), true, cx);
    cx.update(|cx| {
        BrowserTool::settings_changed(cx);
        BrowserTool::settings_changed(cx);
    });
    cx.run_until_parked();

    assert_eq!(installer.calls(), 1);
    assert_eq!(state(cx), BrowserToolState::Ready(binary()));
}

#[gpui::test]
fn a_failed_install_is_retried_when_turned_on_again(cx: &mut TestAppContext) {
    let installer = FakeInstaller::new(None, vec![Err("offline".to_owned()), Ok(binary())]);
    install_tool(installer.clone(), false, cx);

    toggle(true, cx);
    assert_eq!(state(cx), BrowserToolState::Failed);

    toggle(false, cx);
    toggle(true, cx);
    assert_eq!(installer.calls(), 2);
    assert_eq!(state(cx), BrowserToolState::Ready(binary()));
}

#[gpui::test]
fn a_lane_gets_a_launch_only_when_enabled_and_ready(cx: &mut TestAppContext) {
    let lane = Path::new("/work/repo");
    let cases = [
        ("disabled", false, Some(binary()), false),
        ("enabled and ready", true, Some(binary()), true),
        ("enabled, nothing installed", true, None, false),
    ];
    for (label, enabled, cached, launches) in cases {
        install_tool(FakeInstaller::new(cached, vec![Ok(binary())]), enabled, cx);
        let launch = cx.update(|cx| launch_for_lane(lane, cx));
        assert_eq!(launch.is_some(), launches, "{label}");
        if let Some(launch) = launch {
            assert_eq!(launch.command, binary(), "{label}");
            assert!(
                launch
                    .env
                    .iter()
                    .any(|(name, _)| name == browser_tool::SESSION_ENV),
                "{label}: {:?}",
                launch.env
            );
        }
    }
}
