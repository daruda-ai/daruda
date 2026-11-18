use super::*;

fn pr(number: u64) -> PrRef {
    PrRef {
        host: "github.com".into(),
        owner: "o".into(),
        name: "r".into(),
        number,
    }
}

const WT: &str = "/w/feat";

#[test]
fn choosing_unlinking_and_restoring_round_trip_with_history() {
    let dir = tempfile::tempdir().unwrap();
    let wt = Path::new(WT);
    let mut links = PrLinks::default();
    links.found(wt, "feat", pr(3), LinkSource::Branch, 1);
    links.found(wt, "feat", pr(3), LinkSource::Branch, 2);
    links.unlink(wt, "feat", pr(3), LinkSource::Branch, true, 3);
    links.choose(wt, "feat", pr(9), 4);
    save_pr_links_in(dir.path(), &links).unwrap();

    let mut loaded = load_pr_links_in(dir.path()).unwrap();
    let held = loaded.of(wt, "feat").unwrap();
    assert_eq!(held.chosen, Some(pr(9)));
    assert_eq!(held.suppressed, [pr(3)]);
    let history: Vec<_> = held
        .history
        .iter()
        .map(|event| (event.pr.number, event.source, event.unlinked_at))
        .collect();
    assert_eq!(
        history,
        [
            (3, LinkSource::Branch, Some(3)),
            (9, LinkSource::Chosen, None)
        ],
        "found once, then unlinked; then chosen"
    );
    assert!(
        loaded.of(wt, "other").is_none(),
        "another branch starts clean"
    );

    // Unlinking the chosen one drops it; restoring lets the name find again.
    loaded.unlink(wt, "feat", pr(9), LinkSource::Chosen, false, 5);
    loaded.restore(wt, "feat");
    let held = loaded.of(wt, "feat").unwrap();
    assert_eq!((held.chosen.clone(), held.suppressed.len()), (None, 0));
    assert_eq!(held.history.len(), 2, "history survives unlinking");
}

#[test]
fn a_newer_or_damaged_file_is_left_alone_and_history_is_bounded() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        pr_links_path_in(dir.path()),
        r#"{"schema_version": 99, "worktrees": []}"#,
    )
    .unwrap();
    assert!(load_pr_links_in(dir.path()).is_none());
    std::fs::write(pr_links_path_in(dir.path()), "{").unwrap();
    assert!(load_pr_links_in(dir.path()).is_none());

    let mut links = PrLinks::default();
    for n in 0..30 {
        links.choose(Path::new(WT), "feat", pr(n), n as i64);
    }
    assert_eq!(
        links.of(Path::new(WT), "feat").unwrap().history.len(),
        HISTORY_MAX
    );
}

#[test]
fn a_chosen_pull_request_the_name_also_finds_stays_untied() {
    let wt = Path::new(WT);
    let mut links = PrLinks::default();
    links.choose(wt, "feat", pr(7), 1);
    links.unlink(wt, "feat", pr(7), LinkSource::Chosen, true, 2);
    let held = links.of(wt, "feat").unwrap();
    assert_eq!(
        (held.chosen.clone(), held.suppressed.clone()),
        (None, vec![pr(7)])
    );
}

#[test]
fn a_new_link_closes_the_one_it_replaces() {
    let wt = Path::new(WT);
    let mut links = PrLinks::default();
    links.found(wt, "feat", pr(3), LinkSource::Branch, 1);
    links.found(wt, "feat", pr(4), LinkSource::Branch, 2);
    let history = &links.of(wt, "feat").unwrap().history;
    assert_eq!(history[0].unlinked_at, Some(2));
    assert_eq!(history[1].unlinked_at, None);
}
