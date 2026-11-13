//! The process-wide managed agent-browser install, driven by
//! `[browser] enabled`.
//!
//! One install runs at a time for the whole process; restored chat panes
//! reconnect early, so startup checks the cache synchronously.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use daruda_agent::browser_tool::install::{self, InstallError, InstallStage};
use daruda_agent::browser_tool::{self, Asset, McpLaunch, SocketDirError};
use daruda_store::observability::error_report::{ErrorReport, ErrorSeverity};
use daruda_store::observability::log_writer::LogWriter;
use futures::StreamExt as _;
use gpui::{App, Global};

use crate::settings_store::SettingsStore;
use crate::surface::strings::browser_tool as s;
use crate::window_registry::WindowRegistry;

/// Fetches and verifies the tool. Blocking: callers run it off the
/// foreground thread.
pub(crate) trait Installer: Send + Sync {
    /// The installed binary, when the cache is complete and verified.
    fn cached(&self) -> Option<PathBuf>;
    fn install(&self, on_stage: &mut dyn FnMut(InstallStage)) -> Result<PathBuf, InstallError>;
}

/// The release download into the profile's tool directory.
struct Managed {
    tool_dir: PathBuf,
    asset: Asset,
    os: &'static str,
}

impl Installer for Managed {
    fn cached(&self) -> Option<PathBuf> {
        install::cached(&self.tool_dir, &self.asset, self.os)
    }

    fn install(&self, on_stage: &mut dyn FnMut(InstallStage)) -> Result<PathBuf, InstallError> {
        install::install(
            &self.tool_dir,
            &self.asset,
            self.os,
            on_stage,
            &install::HttpFetch,
            &install::ProcessRun,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BrowserToolState {
    Absent,
    Installing(InstallStage),
    Ready(PathBuf),
    Failed,
}

pub(crate) struct BrowserTool {
    /// The last `[browser] enabled` seen. The settings observer runs on every
    /// settings change, so installs and notices follow only its off-to-on edge.
    enabled: bool,
    host: Host,
    /// Notices raised while no workspace window was open, such as the
    /// startup check, which runs before the first window exists.
    undelivered: Vec<ErrorReport>,
}

/// The tool cannot run in this process, or it can be installed.
enum Host {
    /// Notified once per process, the first time the setting turns on.
    Unusable {
        reason: Unusable,
        notified: bool,
    },
    Supported(Supported),
}

enum Unusable {
    /// Upstream ships no release for this host.
    Platform,
    /// The socket directory cannot hold a session socket, for every lane alike.
    SocketDir(SocketDirError),
}

impl Unusable {
    fn report(&self) -> ErrorReport {
        match self {
            Self::Platform => ErrorReport::new(s::unsupported_platform())
                .severity(ErrorSeverity::Warning)
                .with_context("os", std::env::consts::OS)
                .with_context("arch", std::env::consts::ARCH)
                .dedup("browser_tool.unsupported")
                .build(),
            Self::SocketDir(error) => ErrorReport::new(s::socket_dir_invalid())
                .severity(ErrorSeverity::Warning)
                .from_error(error)
                .at(file!(), line!())
                .dedup("browser_tool.socket_dir")
                .build(),
        }
    }
}

pub(crate) struct Supported {
    installer: Arc<dyn Installer>,
    socket_dir: PathBuf,
    state: BrowserToolState,
}

impl Global for BrowserTool {}

impl BrowserTool {
    /// Install the global for this host. Idempotent.
    pub(crate) fn init(cx: &mut App) {
        if cx.has_global::<Self>() {
            return;
        }
        let tool = default_tool(browser_tool::target(
            std::env::consts::OS,
            std::env::consts::ARCH,
            cfg!(target_env = "musl"),
        ));
        cx.set_global(tool);
    }

    /// A host with a release: the cache decides whether it starts `Ready`,
    /// unless `socket_dir` makes every launch fail.
    pub(crate) fn supported(installer: Arc<dyn Installer>, socket_dir: PathBuf) -> Self {
        if let Err(error) = browser_tool::check_socket_dir(&socket_dir, socket_path_limit()) {
            return Self::unusable(Unusable::SocketDir(error));
        }
        let state = match installer.cached() {
            Some(binary) => BrowserToolState::Ready(binary),
            None => BrowserToolState::Absent,
        };
        Self::with_host(Host::Supported(Supported {
            installer,
            socket_dir,
            state,
        }))
    }

    fn unusable(reason: Unusable) -> Self {
        Self::with_host(Host::Unusable {
            reason,
            notified: false,
        })
    }

    /// Starts as off, so a setting already on at startup counts as turning on.
    fn with_host(host: Host) -> Self {
        Self {
            enabled: false,
            host,
            undelivered: Vec::new(),
        }
    }

    /// When the setting turns on, install if nothing usable is there; a
    /// failed install is retried only by turning the setting off and on
    /// again. An install in flight runs to completion even if it turned off.
    pub(crate) fn settings_changed(cx: &mut App) {
        let enabled = SettingsStore::global(cx).user().browser.enabled;
        let Some(tool) = cx.try_global::<Self>() else {
            return;
        };
        if tool.enabled == enabled {
            return;
        }
        let tool = cx.global_mut::<Self>();
        tool.enabled = enabled;
        if !enabled {
            return;
        }
        match &mut tool.host {
            Host::Unusable { reason, notified } => {
                if !std::mem::replace(notified, true) {
                    let report = reason.report();
                    surface(report, cx);
                }
            }
            Host::Supported(supported)
                if matches!(
                    supported.state,
                    BrowserToolState::Absent | BrowserToolState::Failed
                ) =>
            {
                let installer = supported.installer.clone();
                Self::start_install(installer, cx);
            }
            _ => {}
        }
    }

    fn start_install(installer: Arc<dyn Installer>, cx: &mut App) {
        Self::set_state(BrowserToolState::Installing(InstallStage::Binary), cx);
        let (stages, mut stage_rx) = futures::channel::mpsc::unbounded();
        let task = cx.background_executor().spawn(async move {
            installer.install(&mut |stage| {
                // Fails only once the foreground task is gone, i.e. the app quit.
                let _ = stages.unbounded_send(stage);
            })
        });
        cx.spawn(async move |cx| {
            while let Some(stage) = stage_rx.next().await {
                cx.update(|cx| Self::set_state(BrowserToolState::Installing(stage), cx));
            }
            let result = task.await;
            cx.update(|cx| Self::finish_install(result, cx));
        })
        .detach();
    }

    fn finish_install(result: Result<PathBuf, InstallError>, cx: &mut App) {
        let report = match result {
            Ok(binary) => {
                Self::set_state(BrowserToolState::Ready(binary), cx);
                ErrorReport::new(s::installed())
                    .severity(ErrorSeverity::Info)
                    .dedup("browser_tool.installed")
                    .build()
            }
            Err(error) => {
                Self::set_state(BrowserToolState::Failed, cx);
                ErrorReport::new(s::install_failed())
                    .severity(ErrorSeverity::Error)
                    .from_error(&error)
                    .at(file!(), line!())
                    .dedup("browser_tool.install_failed")
                    .build()
            }
        };
        surface(report, cx);
    }

    fn set_state(state: BrowserToolState, cx: &mut App) {
        if let Host::Supported(supported) = &mut cx.global_mut::<Self>().host {
            supported.state = state;
        }
    }
}

/// How to start `agent-browser mcp` for a lane's chat session, when the
/// setting is on and the tool is installed.
pub(crate) fn launch_for_lane(lane_root: &Path, cx: &App) -> Option<McpLaunch> {
    if !SettingsStore::global(cx).user().browser.enabled {
        return None;
    }
    let Host::Supported(Supported {
        socket_dir,
        state: BrowserToolState::Ready(binary),
        ..
    }) = &cx.try_global::<BrowserTool>()?.host
    else {
        return None;
    };
    let launch = browser_tool::launch(binary, socket_dir, lane_root, socket_path_limit());
    // `supported` checked this socket directory, the only input `launch` can
    // reject, so a `Supported` host never gets here.
    debug_assert!(launch.is_ok(), "{launch:?}");
    launch.ok()
}

/// Upstream binds a Unix socket only on Unix hosts.
fn socket_path_limit() -> Option<usize> {
    cfg!(unix).then_some(browser_tool::UNIX_SOCKET_PATH_LIMIT)
}

/// Deliver the notices held for want of a window. Deferred: registration
/// runs inside the new workspace's construction.
pub(crate) fn on_window_registered(cx: &mut App) {
    if cx
        .try_global::<BrowserTool>()
        .is_none_or(|tool| tool.undelivered.is_empty())
    {
        return;
    }
    cx.defer(|cx| {
        let reports = std::mem::take(&mut cx.global_mut::<BrowserTool>().undelivered);
        for report in reports {
            surface(report, cx);
        }
    });
}

/// Toast in the focused workspace (or any open one); held until a workspace
/// window registers when none is open.
fn surface(report: ErrorReport, cx: &mut App) {
    let Some((_, workspace)) =
        WindowRegistry::active_workspace(cx).or_else(|| WindowRegistry::first_workspace(cx))
    else {
        cx.global_mut::<BrowserTool>().undelivered.push(report);
        return;
    };
    let shown = report.clone();
    if workspace
        .update(cx, |ws, cx| ws.report_error(shown, cx))
        .is_err()
    {
        LogWriter::log(report);
    }
}

/// The tool for a host whose release is `target`, kept in `tool_dir`.
fn host_tool(target: Option<Asset>, tool_dir: PathBuf, socket_dir: PathBuf) -> BrowserTool {
    match target {
        Some(asset) => BrowserTool::supported(
            Arc::new(Managed {
                tool_dir,
                asset,
                os: std::env::consts::OS,
            }),
            socket_dir,
        ),
        None => BrowserTool::unusable(Unusable::Platform),
    }
}

#[cfg(not(test))]
fn default_tool(target: Option<Asset>) -> BrowserTool {
    host_tool(
        target,
        daruda_store::persistence::browser_tool_dir(),
        daruda_store::persistence::browser_socket_dir(),
    )
}

/// Test builds never download or read the profile's tool directory; a test
/// that wants a tool builds one with [`host_tool`] or [`BrowserTool::supported`].
#[cfg(test)]
fn default_tool(_target: Option<Asset>) -> BrowserTool {
    BrowserTool::unusable(Unusable::Platform)
}

#[cfg(test)]
mod tests;
