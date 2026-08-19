//! H1 — the config file and the topic (spec §5, §6).

mod common;

use std::os::unix::fs::PermissionsExt;

use common::TempDir;
use hub::config::{Config, DEFAULT_PORT, load_or_create};

#[test]
fn a_missing_config_is_created_with_a_topic_and_mode_0600() {
    let dir = TempDir::new("config-create");
    let path = dir.join("nested/config.toml");
    assert!(!path.exists());

    let config = load_or_create(&path).unwrap();

    assert!(path.is_file(), "the file is created, not just defaulted");
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "the topic is the only secret hub holds");
    assert_eq!(config.port, DEFAULT_PORT);
    assert_eq!(config.ntfy_base, "https://ntfy.sh");
    assert!(config.siblings.is_empty());
    assert!(config.origins.is_empty());

    // Re-reading finds the same topic rather than minting a second one.
    let again = load_or_create(&path).unwrap();
    assert_eq!(again.topic, config.topic);
}

#[test]
fn the_topic_is_twenty_six_base32_characters_and_never_starts_with_a_timestamp() {
    let dir = TempDir::new("config-topic");
    let mut topics = Vec::new();
    for n in 0..8 {
        let path = dir.join(&format!("{n}/config.toml"));
        topics.push(load_or_create(&path).unwrap().topic);
    }

    for topic in &topics {
        let tail = topic
            .strip_prefix("workflow-")
            .unwrap_or_else(|| panic!("{topic} is not workflow-prefixed"));
        assert_eq!(tail.chars().count(), 26, "{topic}");
        assert!(
            tail.chars()
                .all(|c| "0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(c)),
            "{topic} is not Crockford base32"
        );
    }

    // A ULID would share its leading characters across a run of ids minted in
    // the same millisecond — review M-4, and the reason §5 says /dev/urandom.
    let firsts: std::collections::BTreeSet<&str> = topics
        .iter()
        .map(|t| &t["workflow-".len()..][..6])
        .collect();
    assert_eq!(
        firsts.len(),
        topics.len(),
        "leading characters repeat: {topics:?}"
    );
}

#[test]
fn an_existing_config_is_read_verbatim_and_not_rewritten() {
    let dir = TempDir::new("config-read");
    let path = dir.join("config.toml");
    std::fs::write(
        &path,
        r#"# hand written, comments and all
port = 9191
topic = "workflow-0123456789ABCDEFGHJKMNPQ"
siblings = ["http://nuc:8787"]
ntfy_base = "http://127.0.0.1:1"
origins = ["http://macbook.example.ts.net:9191"]
"#,
    )
    .unwrap();
    let before = std::fs::read_to_string(&path).unwrap();

    let config = load_or_create(&path).unwrap();

    assert_eq!(config.port, 9191);
    assert_eq!(config.topic, "workflow-0123456789ABCDEFGHJKMNPQ");
    assert_eq!(config.siblings, vec!["http://nuc:8787"]);
    assert_eq!(config.ntfy_base, "http://127.0.0.1:1");
    assert_eq!(config.origins, vec!["http://macbook.example.ts.net:9191"]);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        before,
        "a complete config is never rewritten — the comments survive"
    );
}

#[test]
fn a_config_without_a_topic_gains_one_and_keeps_its_other_keys() {
    let dir = TempDir::new("config-topicless");
    let path = dir.join("config.toml");
    std::fs::write(&path, "port = 9292\nsiblings = [\"http://nuc:8787\"]\n").unwrap();

    let config = load_or_create(&path).unwrap();

    assert!(config.topic.starts_with("workflow-"));
    assert_eq!(config.port, 9292);
    let written = std::fs::read_to_string(&path).unwrap();
    assert!(
        written.contains(&config.topic),
        "the topic is persisted: {written}"
    );
    assert!(
        written.contains("9292"),
        "the other keys survive: {written}"
    );
    assert_eq!(load_or_create(&path).unwrap().topic, config.topic);
}

#[test]
fn an_unknown_key_is_an_error_rather_than_a_silent_default() {
    let dir = TempDir::new("config-typo");
    let path = dir.join("config.toml");
    // `ntfy-base` instead of `ntfy_base` would otherwise leave hub publishing
    // to ntfy.sh while the file says it is pointed somewhere else.
    std::fs::write(&path, "ntfy-base = \"http://127.0.0.1:1\"\n").unwrap();

    let err = format!("{:#}", load_or_create(&path).unwrap_err());
    assert!(err.contains("ntfy-base"), "{err}");
}

#[test]
fn an_empty_file_is_all_defaults_plus_a_topic() {
    let dir = TempDir::new("config-empty");
    let path = dir.join("config.toml");
    std::fs::write(&path, "").unwrap();

    let config = load_or_create(&path).unwrap();

    assert_eq!(config.port, DEFAULT_PORT);
    assert_eq!(config.ntfy_base, "https://ntfy.sh");
    assert!(config.topic.starts_with("workflow-"));
}

#[test]
fn a_trailing_slash_on_ntfy_base_does_not_become_a_double_slash() {
    let config = Config {
        ntfy_base: "http://127.0.0.1:9/".into(),
        topic: "workflow-AAAAAAAAAAAAAAAAAAAAAAAAAA".into(),
        ..Config::default()
    };
    assert_eq!(
        config.ntfy_url(),
        "http://127.0.0.1:9/workflow-AAAAAAAAAAAAAAAAAAAAAAAAAA"
    );
}
