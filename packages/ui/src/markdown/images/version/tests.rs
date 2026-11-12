use super::*;

#[test]
fn negative_versions_recover_and_length_changes_invalidate() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("image");
    assert_eq!(Version::read(&path), Version::Read);
    std::fs::write(&path, b"first").unwrap();
    let first = Version::read(&path);
    assert!(matches!(first, Version::File { len: 5, .. }));
    std::fs::write(&path, b"replacement").unwrap();
    assert_ne!(first, Version::read(&path));
    assert_eq!(Version::read(dir.path()), Version::NotFile);
}
