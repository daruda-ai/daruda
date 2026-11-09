//! Run ids: the run directory's name, which encodes when the run started.

use gpui::Context;

use crate::workspace::Workspace;

/// A run id that sorts in the order runs were made.
///
/// Retention sweeps run directories by name and only that ordering makes
/// the sweep chronological, so this property is load-bearing rather than
/// cosmetic. Millisecond, pid and counter are each fixed-width hex: the
/// clock orders across runs, the pid separates two apps that started in
/// the same millisecond, and the counter separates two runs from one app.
pub(in crate::workspace) fn run_id(millis: u128, pid: u32, counter: u32) -> String {
    format!("{millis:016x}-{pid:08x}-{counter:04x}")
}

/// When a run started, read back out of its id.
///
/// Deliberately here rather than beside the reader: the id's layout is the
/// host's, and `daruda_flow` says so — its retention sweep notes that "a
/// host using a different id scheme would silently have this delete the
/// wrong ones". One place builds the format and reads it, so the two
/// cannot drift.
///
/// `None` for any name this host did not make (a hand-created directory,
/// or a run from a future scheme), which the caller shows without a time
/// rather than guessing one from the directory's mtime.
pub(in crate::workspace) fn run_started_at(run_id: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let (millis, rest) = run_id.split_once('-')?;
    // Both remaining fields must be there: a bare 16-hex name is not this
    // scheme, and accepting it would date arbitrary directories.
    if millis.len() != 16 || !rest.contains('-') {
        return None;
    }
    chrono::DateTime::from_timestamp_millis(i64::from_str_radix(millis, 16).ok()?)
}
impl Workspace {
    pub(in crate::workspace) fn next_run_id(&mut self, cx: &mut Context<Self>) -> String {
        let counter = crate::runs::RunsHost::next_flow_run_id(cx);
        run_id(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or_default(),
            std::process::id(),
            counter,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The panel dates a past run by decoding its directory name, so the
    /// two halves of that format have to stay one thing. Change the layout
    /// in `run_id` alone and every past run loses its time.
    #[test]
    fn a_run_id_carries_back_the_time_it_was_made() {
        let millis = 1_786_000_000_000u128;
        let decoded = run_started_at(&run_id(millis, 42, 1)).expect("decodes");
        assert_eq!(decoded.timestamp_millis(), millis as i64);
    }

    /// Anything this host did not name gets no time rather than a wrong
    /// one — a user's own directory under `flow-runs/` must not be dated
    /// from whatever its name happens to parse as.
    #[test]
    fn a_name_that_is_not_a_run_id_decodes_to_nothing() {
        for name in [
            "scratch",
            "",
            // Right shape, not hex.
            "zzzzzzzzzzzzzzzz-0000002a-0001",
            // Hex, but not the full fixed width.
            "1786-0000002a-0001",
            // The clock alone is not the scheme.
            "0000019fbe1f8a00",
        ] {
            assert!(run_started_at(name).is_none(), "{name} was dated");
        }
    }

    /// Retention deletes the oldest runs by sorting directory names, so a
    /// later run must sort after an earlier one — including across a clock
    /// that did not move between them.
    #[test]
    fn run_ids_sort_in_the_order_they_were_made() {
        let ids = [
            run_id(1_700_000_000_000, 42, 1),
            run_id(1_700_000_000_000, 42, 2),
            run_id(1_700_000_000_001, 42, 1),
            run_id(1_700_000_001_000, 7, 1),
        ];
        let mut sorted = ids.to_vec();
        sorted.sort();
        assert_eq!(sorted, ids);
    }

    /// Two apps starting a run in the same millisecond must not land on the
    /// same directory — the counter is per-process and would collide.
    #[test]
    fn two_processes_in_one_millisecond_get_different_ids() {
        assert_ne!(
            run_id(1_700_000_000_000, 42, 1),
            run_id(1_700_000_000_000, 43, 1)
        );
    }
}
