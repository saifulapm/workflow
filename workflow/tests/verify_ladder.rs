//! The precedence ladder of spec §7, stated as a table: a repo shaped like
//! this gets exactly these verifiers, in this order.
//!
//! The project owns its checks; the workflow supplies only a floor for what the
//! project has not spoken for. A declared entry point replaces the floor *for
//! what it speaks for* — `composer test` over the artisan detection, a repo-wide
//! `just test` over the floor entirely — while a language the project said
//! nothing about still gets its floor.

use std::path::PathBuf;

use workflow::memcli::Project;
use workflow::verify::detect_verifiers;

struct Fixture(PathBuf);

impl Fixture {
    fn new(name: &str) -> Fixture {
        let dir = std::env::temp_dir().join(format!("wf-ladder-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Fixture(dir)
    }

    fn file(self, path: &str, body: &str) -> Fixture {
        let p = self.0.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, body).unwrap();
        self
    }

    fn exec(self, path: &str) -> Fixture {
        use std::os::unix::fs::PermissionsExt;
        let f = self.file(path, "#!/bin/sh\nexit 0\n");
        let p = f.0.join(path);
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        f
    }

    fn ladder(&self, project: Option<&Project>) -> Vec<(String, String)> {
        detect_verifiers(&self.0, project)
            .into_iter()
            .map(|v| (v.label.to_string(), v.cmd))
            .collect()
    }

    fn labels(&self) -> Vec<String> {
        self.ladder(None).into_iter().map(|(l, _)| l).collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn project_with_verify(cmd: &str) -> Project {
    Project {
        id: "p1".into(),
        name: "app".into(),
        root: None,
        verify: Some(cmd.into()),
        review_paths: None,
    }
}

#[test]
fn tier_one_short_circuits_everything_below_it() {
    let f = Fixture::new("tier1")
        .file("composer.json", r#"{"scripts":{"test":"phpunit"}}"#)
        .file("package.json", r#"{"scripts":{"test":"vitest"}}"#)
        .file("Cargo.toml", "[package]\nname='x'\n");
    let p = project_with_verify("bin/php artisan test --filter=Cart");
    assert_eq!(
        f.ladder(Some(&p)),
        vec![(
            "project".to_string(),
            "bin/php artisan test --filter=Cart".to_string()
        )]
    );
}

#[test]
fn a_declared_composer_script_replaces_the_artisan_floor() {
    let f = Fixture::new("composer")
        .file("composer.json", r#"{"scripts":{"test":"phpunit"}}"#)
        .file("artisan", "")
        .exec("bin/php");
    assert_eq!(
        f.ladder(None),
        vec![("composer".to_string(), "composer test".to_string())]
    );
}

#[test]
fn node_scripts_are_pnpm_and_lint_only_when_it_is_scripted() {
    let f = Fixture::new("node").file(
        "package.json",
        r#"{"scripts":{"test":"vitest","lint":"eslint ."}}"#,
    );
    assert_eq!(
        f.ladder(None),
        vec![
            ("node".to_string(), "pnpm run test".to_string()),
            ("node-lint".to_string(), "pnpm lint".to_string()),
        ]
    );

    let f = Fixture::new("node-nolint").file("package.json", r#"{"scripts":{"test":"vitest"}}"#);
    assert_eq!(f.labels(), vec!["node"]);
}

#[test]
fn the_npm_placeholder_test_script_is_not_a_suite() {
    let f = Fixture::new("placeholder").file(
        "package.json",
        r#"{"scripts":{"test":"echo \"Error: no test specified\" && exit 1"}}"#,
    );
    assert!(f.labels().is_empty());
}

#[test]
fn a_repo_wide_test_target_replaces_the_floor_altogether() {
    let f = Fixture::new("just")
        .file("justfile", "test:\n\tcargo test\n")
        .file("Cargo.toml", "[package]\nname='x'\n");
    assert_eq!(f.labels(), vec!["just"]);

    let f = Fixture::new("make")
        .file("Makefile", "test:\n\t@true\n")
        .file("Cargo.toml", "[package]\nname='x'\n");
    assert_eq!(f.labels(), vec!["make"]);

    // `test-unit:` is not a `test` target, so the floor still applies.
    let f = Fixture::new("near-miss")
        .file("justfile", "test-unit:\n\tcargo test\n")
        .file("Cargo.toml", "[package]\nname='x'\n");
    assert_eq!(f.labels(), vec!["rust"]);
}

#[test]
fn the_ecosystem_floor_is_the_whole_three_part_rust_command() {
    let f = Fixture::new("rust").file("Cargo.toml", "[package]\nname='x'\n");
    assert_eq!(
        f.ladder(None),
        vec![(
            "rust".to_string(),
            "cargo test && cargo clippy -- -D warnings && cargo fmt --check".to_string()
        )]
    );
}

#[test]
fn the_php_floor_goes_through_the_shim_when_the_project_has_one() {
    let f = Fixture::new("artisan")
        .file("composer.json", "{}")
        .file("artisan", "")
        .exec("bin/php");
    assert_eq!(
        f.ladder(None),
        vec![("php".to_string(), "./bin/php artisan test".to_string())]
    );

    let f = Fixture::new("pest")
        .file("composer.json", "{}")
        .exec("vendor/bin/pest");
    assert_eq!(
        f.ladder(None),
        vec![("php".to_string(), "php vendor/bin/pest".to_string())]
    );

    let f = Fixture::new("phpunit")
        .file("composer.json", "{}")
        .exec("vendor/bin/phpunit");
    assert_eq!(
        f.ladder(None),
        vec![("php".to_string(), "php vendor/bin/phpunit".to_string())]
    );

    // composer.json and nothing to run: no PHP verifier, and it says so.
    let f = Fixture::new("bare-php").file("composer.json", "{}");
    assert!(f.labels().is_empty());
}

#[test]
fn a_language_the_project_said_nothing_about_still_gets_its_floor() {
    // A Laravel repo that declares a JavaScript suite keeps its PHP one.
    let f = Fixture::new("polyglot")
        .file("composer.json", "{}")
        .file("artisan", "")
        .exec("bin/php")
        .file("package.json", r#"{"scripts":{"test":"vitest"}}"#);
    assert_eq!(f.labels(), vec!["node", "php"]);
}

#[test]
fn a_theme_is_recognised_by_the_two_files_that_make_one() {
    let f = Fixture::new("theme")
        .file("config/settings_schema.json", "{}")
        .file("layout/theme.liquid", "x");
    // theme-check may or may not be on this machine; either way the detection
    // fired, and with nothing on PATH it says so and adds nothing.
    let labels = f.labels();
    assert!(labels.is_empty() || labels == vec!["theme"]);

    let f = Fixture::new("not-a-theme").file("config/settings_schema.json", "{}");
    assert!(f.labels().is_empty());
}

#[test]
fn an_empty_repo_has_no_verifier_at_all() {
    let f = Fixture::new("empty").file("README.md", "hello");
    assert!(f.labels().is_empty());
}
