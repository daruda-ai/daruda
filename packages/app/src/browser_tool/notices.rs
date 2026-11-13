//! Delivery of the tool's notices: a toast in an open workspace, or held
//! until the first workspace window registers.

use daruda_store::observability::error_report::ErrorReport;
use daruda_store::observability::log_writer::LogWriter;
use gpui::App;

use super::BrowserTool;
use crate::window_registry::WindowRegistry;

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
pub(super) fn surface(report: ErrorReport, cx: &mut App) {
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

#[cfg(test)]
mod tests;
