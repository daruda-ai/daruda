use super::*;
fn seed(root: &Path, value: serde_json::Value) -> PathBuf {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(root.join("dot.svg"),r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><circle cx="8" cy="8" r="3"/></svg>"##).unwrap();
    let path = root.join("theme.json");
    std::fs::write(&path, value.to_string()).unwrap();
    path
}
fn manifest() -> serde_json::Value {
    serde_json::json!({"version":1,"id":"sample","name":"Sample","states":{"working":{"frames":["dot.svg","dot.svg"],"ticks_per_frame":2,"colors":{"dark":"#a2d7ec","light":"#34738a"}}}})
}
#[test]
fn packages_load_colors_and_frames_with_shared_clock_timing() {
    let dir = tempfile::tempdir().unwrap();
    let path = seed(dir.path(), manifest());
    let theme = package(&path, "user").unwrap();
    assert_eq!(theme.id, "user/sample");
    let art = &theme.states[&State::Working];
    assert_eq!(art.colors, Some((0xa2d7ec, 0x34738a)));
    assert!(std::ptr::eq(art.frame(0).unwrap(), art.frame(1).unwrap()));
    assert!(!std::ptr::eq(art.frame(1).unwrap(), art.frame(2).unwrap()));
    assert!(std::ptr::eq(art.frame(0).unwrap(), art.frame(4).unwrap()));
}
#[test]
fn malformed_packages_do_not_hide_good_providers() {
    let dir = tempfile::tempdir().unwrap();
    seed(&dir.path().join("good"), manifest());
    let mut bad = manifest();
    bad["version"] = 2.into();
    seed(&dir.path().join("bad"), bad);
    let (catalog, errors) = catalog(&[("user".into(), dir.path().into())]);
    assert_eq!(catalog.themes.len(), 1);
    assert_eq!(errors.len(), 1);
}
#[test]
fn invalid_state_timing_paths_and_colors_are_rejected() {
    for (key, value) in [
        ("ticks_per_frame", serde_json::json!(0)),
        ("frames", serde_json::json!(["../dot.svg"])),
        (
            "colors",
            serde_json::json!({"dark":"blue","light":"#ffffff"}),
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let mut spec = manifest();
        spec["states"]["working"][key] = value;
        assert!(package(&seed(dir.path(), spec), "user").is_err());
    }
    let dir = tempfile::tempdir().unwrap();
    let mut spec = manifest();
    spec["states"]["idle"] = spec["states"]["working"].take();
    spec["states"].as_object_mut().unwrap().remove("working");
    assert!(package(&seed(dir.path(), spec), "user").is_err());
}
#[test]
fn svg_edits_invalidate_the_renderer_cache_and_external_content_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = seed(dir.path(), manifest());
    let before = package(&path, "user").unwrap().states[&State::Working].frames[0]
        .key
        .clone();
    let svg = dir.path().join("dot.svg");
    let bytes = std::fs::read_to_string(&svg)
        .unwrap()
        .replace("r=\"3\"", "r=\"4\"");
    std::fs::write(&svg, bytes).unwrap();
    let after = package(&path, "user").unwrap().states[&State::Working].frames[0]
        .key
        .clone();
    assert_ne!(before, after);
    std::fs::write(
        &svg,
        r#"<svg xmlns="http://www.w3.org/2000/svg"><image href="outside.svg"/></svg>"#,
    )
    .unwrap();
    assert!(package(&path, "user").is_err());
}
#[test]
fn manifests_without_extends_inherit_dots() {
    let dir = tempfile::tempdir().unwrap();
    let theme = package(&seed(dir.path(), manifest()), "user").unwrap();
    assert_eq!(theme.base, Base::Dots);
    let mut icons = manifest();
    icons["extends"] = "icons".into();
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        package(&seed(dir.path(), icons), "user").unwrap().base,
        Base::Icons
    );
}
#[test]
fn catalog_limit_stops_every_later_provider_with_one_error() {
    let first = tempfile::tempdir().unwrap();
    for index in 0..=MAX_THEMES {
        let mut spec = manifest();
        spec["id"] = format!("theme-{index:03}").into();
        seed(&first.path().join(format!("theme-{index:03}")), spec);
    }
    let second = tempfile::tempdir().unwrap();
    seed(&second.path().join("late"), manifest());
    let (catalog, errors) = catalog(&[
        ("user".into(), first.path().into()),
        ("plugin/late".into(), second.path().into()),
    ]);
    assert_eq!(catalog.themes.len(), MAX_THEMES);
    assert_eq!(errors.len(), 1);
}
