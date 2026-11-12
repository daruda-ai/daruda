use std::time::Duration;

use daruda_config::GithubConfig;

#[test]
fn an_absent_section_is_enabled_at_thirty_seconds() {
    let parsed: GithubConfig = toml::from_str("").unwrap();
    assert!(parsed.enabled);
    assert_eq!(parsed.interval(), Duration::from_secs(30));
}

#[test]
fn the_interval_never_drops_below_the_floor() {
    let parsed: GithubConfig = toml::from_str("poll_secs = 1").unwrap();
    assert_eq!(
        parsed.interval(),
        Duration::from_secs(GithubConfig::MIN_POLL_SECS)
    );
}

#[test]
fn the_section_reads_from_the_full_config() {
    let config: daruda_config::Config = toml::from_str("[github]\nenabled = false").unwrap();
    assert!(!config.github.enabled);
}
