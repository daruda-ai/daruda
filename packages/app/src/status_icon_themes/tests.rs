use super::*;
#[test]
fn only_installed_user_plugins_are_global_theme_providers() {
    use daruda_agent::skills::plugins::PluginInstall;
    let plugin = |scope: &str| PluginInstall {
        id: "sample@market".into(),
        scope: scope.into(),
        install_path: PathBuf::from("plugin"),
        version: "1".into(),
    };
    let found = sources(
        Path::new("user"),
        &[plugin("project"), plugin("local"), plugin("user")],
    );
    assert_eq!(found.len(), 2);
    assert_eq!(found[1].0, "plugin/sample@market");
    assert_eq!(
        found[1].1,
        PathBuf::from("plugin/daruda/status-icon-themes")
    );
}

#[test]
fn bundled_themes_survive_user_theme_edits_and_removal() {
    let dir = tempfile::tempdir().unwrap();
    let package = dir.path().join("cats");
    std::fs::create_dir(&package).unwrap();
    let path = package.join("theme.json");
    let manifest = serde_json::json!({
        "version": 1, "id": "cats", "name": "My cats",
        "states": {"working": {"colors": {"dark": "#123456", "light": "#abcdef"}}}
    });
    std::fs::write(&path, manifest.to_string()).unwrap();
    let sources = vec![("user".into(), dir.path().to_path_buf())];
    let empty = load(&[]);
    assert_eq!(empty.choices().len(), 3);
    let original = empty.themes["builtin/cats"].states[&State::Working].frames[0].layers[0]
        .key
        .clone();
    let installed = load(&sources);
    assert_eq!(installed.themes["user/cats"].name, "My cats");
    assert_eq!(installed.choices().len(), 4);
    for contents in ["invalid manifest", ""] {
        if contents.is_empty() {
            std::fs::remove_dir_all(&package).unwrap();
        } else {
            std::fs::write(&path, contents).unwrap();
        }
        let catalog = load(&sources);
        assert!(!catalog.themes.contains_key("user/cats"));
        assert_eq!(catalog.choices().len(), 3);
        assert_eq!(
            catalog.themes["builtin/cats"].states[&State::Working].frames[0].layers[0].key,
            original
        );
    }
}
