//! Recovery must preserve live repositories and delete only confirmed leftovers.

use crate::fixture::{git, init, open};
use daruda_project::lane::git::LaneRemovalState;

fn repository(root: &std::path::Path) {
    init(root);
    git(root, &["commit", "--allow-empty", "-qm", "initial"]);
}

#[test]
fn removal_inspection_distinguishes_registration_and_disk_state() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("main");
    repository(&root);
    let repo = open(&root);
    let lane = temp.path().join("worktree");
    repo.add_lane(&lane, Some("feature"), None).unwrap();
    assert_eq!(
        repo.lane_removal_state(&lane).unwrap(),
        LaneRemovalState::Registered
    );
    std::fs::remove_dir_all(&lane).unwrap();
    assert_eq!(
        repo.lane_removal_state(&lane).unwrap(),
        LaneRemovalState::Registered
    );
    repo.remove_lane(&lane, false).unwrap();
    assert_eq!(
        repo.lane_removal_state(&lane).unwrap(),
        LaneRemovalState::Missing
    );
    std::fs::create_dir(&lane).unwrap();
    assert_eq!(
        repo.lane_removal_state(&lane).unwrap(),
        LaneRemovalState::Unregistered
    );
    std::fs::write(
        lane.join("uncommitted.txt"),
        "keep until explicitly removed",
    )
    .unwrap();
    repo.remove_orphaned_lane(&lane).unwrap();
    assert!(!lane.exists());
    repo.remove_orphaned_lane(&lane).unwrap();
}

#[test]
fn orphan_cleanup_refuses_repositories_nested_repositories_and_registered_paths() {
    for case in [
        "main",
        "ancestor",
        "child",
        "independent",
        "nested",
        "bare",
        "nested_bare",
        "registered",
        "replaced",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("main");
        repository(&root);
        let repo = open(&root);
        let lane = temp.path().join("leftover");
        std::fs::create_dir(&lane).unwrap();
        let target = match case {
            "main" => root.clone(),
            "ancestor" => temp.path().to_path_buf(),
            "child" => {
                let child = root.join("child");
                std::fs::create_dir(&child).unwrap();
                child
            }
            "independent" => {
                repository(&lane);
                lane.clone()
            }
            "nested" => {
                repository(&lane.join("nested"));
                lane.clone()
            }
            "bare" => {
                git(&lane, &["init", "--bare", "-q"]);
                lane.clone()
            }
            "nested_bare" => {
                let nested = lane.join("nested");
                std::fs::create_dir(&nested).unwrap();
                git(&nested, &["init", "--bare", "-q"]);
                lane.clone()
            }
            "registered" => {
                std::fs::remove_dir(&lane).unwrap();
                repo.add_lane(&lane, Some("feature"), None).unwrap();
                lane.clone()
            }
            "replaced" => {
                assert_eq!(
                    repo.lane_removal_state(&lane).unwrap(),
                    LaneRemovalState::Unregistered
                );
                repository(&lane);
                lane.clone()
            }
            _ => panic!("unknown fixture"),
        };
        assert!(repo.remove_orphaned_lane(&target).is_err(), "{case}");
        assert!(target.exists(), "{case} must survive");
        assert!(
            root.join(".git").exists(),
            "{case} must preserve the original repository"
        );
    }
}

#[test]
fn orphan_cleanup_allows_a_dangling_link_to_this_repository_only() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("main");
    repository(&root);
    let repo = open(&root);
    let lane = temp.path().join("leftover 한글 ' $ `");
    repo.add_lane(&lane, Some("feature"), None).unwrap();
    let dirs = open(&lane).dirs().unwrap();
    std::fs::remove_dir_all(&dirs.git_dir).unwrap();
    assert_eq!(
        repo.lane_removal_state(&lane).unwrap(),
        LaneRemovalState::Unregistered
    );
    repo.remove_orphaned_lane(&lane).unwrap();
    assert!(!lane.exists());
    assert!(repo.has_local_branch("feature").unwrap());
}

#[test]
fn orphan_cleanup_requires_a_usable_repository_even_when_target_is_missing() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("missing");
    let repo = open(temp.path());
    assert!(repo.lane_removal_state(&missing).is_err());
    assert!(repo.remove_orphaned_lane(&missing).is_err());
}

#[cfg(unix)]
#[test]
fn orphan_cleanup_does_not_follow_a_target_symlink() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("main");
    repository(&root);
    let outside = temp.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    let link = temp.path().join("leftover");
    daruda_core::path::symlink(&outside, &link).unwrap();
    assert!(open(&root).remove_orphaned_lane(&link).is_err());
    assert!(outside.exists());
    assert!(std::fs::symlink_metadata(&link).is_ok());
}

#[cfg(unix)]
#[test]
fn orphan_cleanup_preserves_bare_repositories_with_symlinked_storage() {
    for name in ["objects", "refs"] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("main");
        repository(&root);
        let bare = temp.path().join("bare");
        std::fs::create_dir(&bare).unwrap();
        git(&bare, &["init", "--bare", "-q"]);
        let storage = temp.path().join("storage");
        std::fs::rename(bare.join(name), &storage).unwrap();
        daruda_core::path::symlink(&storage, bare.join(name)).unwrap();
        assert_eq!(git(&bare, &["rev-parse", "--is-bare-repository"]), "true");
        assert!(open(&root).remove_orphaned_lane(&bare).is_err(), "{name}");
        assert!(bare.join("HEAD").exists(), "{name}");
    }
}

#[test]
fn removal_inspection_preserves_unusual_worktree_path_characters() {
    for name in ["한글 ' $ `", "tab\tname", "line\nname", "trailing space "] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("main");
        repository(&root);
        let lane = temp.path().join(name);
        let repo = open(&root);
        repo.add_lane(&lane, Some("feature"), None).unwrap();
        assert_eq!(
            repo.lane_removal_state(&lane).unwrap(),
            LaneRemovalState::Registered,
            "{name:?}"
        );
        assert!(repo.remove_orphaned_lane(&lane).is_err(), "{name:?}");
        repo.remove_lane(&lane, false).unwrap();
        assert!(!lane.exists(), "{name:?}");
    }
}
