use super::*;

fn holds(held: &[(&str, Option<u64>)]) -> DirectoryHolds {
    DirectoryHolds {
        holds: held
            .iter()
            .map(|(id, until)| Hold {
                run_id: (*id).to_owned(),
                run_dir: Some(PathBuf::from("/runs").join(id)),
                until: *until,
            })
            .collect(),
    }
}

/// Only dated holds carry a marker: a new or moved deadline marks its
/// directory, and a hold that ends or loses its deadline clears it.
#[test]
fn marker_changes_follow_the_dated_holds() {
    let before = holds(&[("kept", Some(5)), ("moved", Some(5)), ("gone", Some(5))]);
    let after = holds(&[
        ("kept", Some(5)),
        ("moved", Some(9)),
        ("new", Some(7)),
        ("open", None),
    ]);
    let mut changes = before.marker_changes(&after);
    changes.sort();
    assert_eq!(
        changes,
        [
            (PathBuf::from("/runs/gone"), None),
            (PathBuf::from("/runs/moved"), Some(9)),
            (PathBuf::from("/runs/new"), Some(7)),
        ]
    );
    let reopened = holds(&[("kept", None)]);
    assert_eq!(
        before.marker_changes(&reopened).len(),
        3,
        "an open run holds without a deadline, so its marker is cleared"
    );
}
