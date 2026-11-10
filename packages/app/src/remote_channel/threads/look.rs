//! How a pane thread's root looks on Slack: its name in bold, its subject,
//! its status, and a stop button while there is work to stop. One builder for
//! the root's first post and every later edit, so the two cannot drift.

use super::RootName;
use super::status::ShownStatus;
use crate::remote_channel::transport::slack::threads::RootView;
use crate::surface::strings as s;

pub(super) struct RootLook {
    title: String,
    subject: Option<String>,
    separator: String,
    status: Option<(&'static str, String)>,
    stop: Option<(String, String)>,
}

impl RootLook {
    pub(super) fn new(name: &RootName, shown: Option<ShownStatus>, id: &str) -> Self {
        Self {
            title: name.title.clone(),
            subject: name.subject.clone(),
            separator: s::remote_channel::thread_detail_separator(),
            status: shown.map(ShownStatus::view),
            stop: shown.filter(|shown| shown.is_running()).map(|_| {
                (
                    s::remote_channel::thread_stop_button(),
                    super::stop::token(id),
                )
            }),
        }
    }

    pub(super) fn view(&self) -> RootView<'_> {
        RootView {
            title: &self.title,
            subject: self.subject.as_deref(),
            separator: &self.separator,
            status: self
                .status
                .as_ref()
                .map(|(emoji, label)| (*emoji, label.as_str())),
            stop: self
                .stop
                .as_ref()
                .map(|(label, value)| (label.as_str(), value.as_str())),
        }
    }
}
