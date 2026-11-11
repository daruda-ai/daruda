//! Remote-channel strings that format an argument first.

/// A pane thread's working status, with the local time the work began.
pub(crate) fn thread_status_working(since: chrono::DateTime<chrono::Utc>) -> String {
    let pattern = crate::surface::strings::timestamp::time_of_day();
    let time = since
        .with_timezone(&chrono::Local)
        .format(&pattern)
        .to_string();
    rust_i18n::t!("remote_channel.thread_status_working", time = time).into_owned()
}

/// A tap's result on the app home, after the local time it came to.
pub(crate) fn home_result(at: chrono::DateTime<chrono::Utc>, text: &str) -> String {
    let pattern = crate::surface::strings::timestamp::time_of_day();
    let time = at
        .with_timezone(&chrono::Local)
        .format(&pattern)
        .to_string();
    rust_i18n::t!("remote_channel.home_result", time = time, text = text).into_owned()
}
