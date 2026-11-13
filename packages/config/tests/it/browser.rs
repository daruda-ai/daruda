use daruda_config::{Config, project::ProjectConfig};

#[test]
fn an_absent_section_is_disabled() {
    let config: Config = toml::from_str("").unwrap();
    assert!(!config.browser.enabled);
}

#[test]
fn the_section_reads_from_the_full_config() {
    let config: Config = toml::from_str("[browser]\nenabled = true").unwrap();
    assert!(config.browser.enabled);
}

#[test]
fn a_project_layer_cannot_turn_the_tool_on() {
    // A repository must not be able to start a download: `[browser]` is user-only.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("project.toml");
    std::fs::write(&path, "[browser]\nenabled = true\n").unwrap();
    let project = ProjectConfig::load_from(&path);

    assert!(!project.overrides_anything());
    assert!(!Config::default().resolve(&project).browser.enabled);
}
