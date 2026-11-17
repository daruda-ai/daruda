//! `WorkspaceStore` persistence through its public API.

use std::collections::BTreeMap;

use daruda_store::project::{WorkspaceStore, WorkspaceUuid};

use crate::support::{sample_project, sample_workspace};

#[test]
fn snapshot_history_is_workspace_scoped_and_intentional_deletion_is_final() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceStore::in_directory(dir.path());
    let a = WorkspaceUuid::new();
    let b = WorkspaceUuid::new();
    let snapshots = BTreeMap::from([(3, "한글🙂".into())]);
    store.save_terminal_snapshots(a, &snapshots).unwrap();
    store
        .save_terminal_snapshots(a, &BTreeMap::from([(3, "next".into())]))
        .unwrap();
    assert!(store.load_terminal_snapshots(b).unwrap().is_empty());
    store.delete_terminal_snapshots(a).unwrap();
    assert!(store.load_terminal_snapshots(a).unwrap().is_empty());
}

#[test]
fn incomplete_workspace_is_preserved_instead_of_partially_restored() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceStore::in_directory(dir.path());
    let project = sample_project();
    let workspace = sample_workspace(project.uuid);
    store.save_workspace(&workspace).unwrap();
    assert!(store.load_complete_workspace(workspace.uuid).is_err());
    assert_eq!(
        store.load_workspace(workspace.uuid),
        Some(workspace.clone())
    );
    store.save_project(&project).unwrap();
    assert_eq!(
        store.load_complete_workspace(workspace.uuid).unwrap(),
        Some((workspace, vec![project]))
    );
}

#[test]
fn the_pull_requests_layout_round_trips_and_older_files_read_as_default() {
    use daruda_store::project::{PullRequestsLayout, WorkspaceState};

    let project = sample_project();
    let mut workspace = sample_workspace(project.uuid);
    let json = serde_json::to_value(&workspace).unwrap();
    assert!(
        json["pull_requests"].as_object().unwrap().is_empty(),
        "defaults write nothing: {json}"
    );
    let mut older = json.clone();
    older.as_object_mut().unwrap().remove("pull_requests");
    let read: WorkspaceState = serde_json::from_value(older).unwrap();
    assert_eq!(read.pull_requests, PullRequestsLayout::default());

    workspace.pull_requests = PullRequestsLayout {
        list_width: Some(480.0),
        list_hidden: true,
    };
    let text = serde_json::to_string(&workspace).unwrap();
    let back: WorkspaceState = serde_json::from_str(&text).unwrap();
    assert_eq!(back.pull_requests, workspace.pull_requests);
}
