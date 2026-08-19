//! H7 — the unit file and `GET /subscribe`.
//!
//! The unit is checked as a file, not by running `systemctl`: the brief wires
//! nothing, and every line of §7's unit is a fix for something the cold review
//! found, so the file's contents are the deliverable. `hub/TESTING.md` §3.3
//! carries the half that needs a live systemd.

mod common;

use std::path::PathBuf;

use common::{Hub, TempDir, body_of, status_of};

fn unit() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("hub.service");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn the_unit_carries_every_line_that_was_missing_from_draft_one() {
    let unit = unit();

    // Without [Install], `systemctl --user enable` refuses outright: "the unit
    // files have no installation config".
    assert!(unit.contains("[Install]"), "{unit}");
    assert!(unit.contains("WantedBy=default.target"), "{unit}");

    // Without this, a boot-started user manager has neither ~/.cargo/bin nor
    // mise's shims, every `mem` shell-out fails, and hub serves the degraded
    // page silently — on the one path that matters.
    assert!(
        unit.contains("Environment=PATH=%h/.cargo/bin:/usr/local/bin:/usr/bin"),
        "{unit}"
    );

    // `on-failure` does not restart a clean exit.
    assert!(unit.contains("Restart=always"), "{unit}");
    assert!(unit.contains("RestartSec=5"), "{unit}");

    assert!(unit.contains("ExecStart=%h/.cargo/bin/hub"), "{unit}");
    assert!(unit.contains("Type=simple"), "{unit}");
    assert!(unit.contains("After=network-online.target"), "{unit}");
    assert!(
        unit.contains("Description=hub — mem's phone-facing view"),
        "{unit}"
    );
}

#[test]
fn hub_installs_nothing_of_its_own_when_it_runs() {
    // This used to read the *real* `$HOME` and assert that
    // `~/.config/systemd/user/hub.service` did not exist. That is not something
    // a test can own: `TESTING.md` §4.3 tells the reader to install exactly
    // that file, so the moment they follow their own instructions the suite is
    // red for ever, on the machine where running it matters most — 109 pass, 1
    // fail, and the failure was detecting a *correct* deployment (review M-4).
    //
    // What the suite can own is hub's own footprint: running it creates its
    // config and its state directory, and installs no unit anywhere. The unit
    // is a file in the repository, and the four tests around this one check
    // its contents.
    let dir = TempDir::new("footprint");
    let home = dir.join("home");
    let hub = Hub::spawn(&home, &[], &["--port", "0"]);
    assert_eq!(status_of(&hub.get("/")), 200);

    let installed = home.join("config/systemd/user/hub.service");
    assert!(
        !installed.exists(),
        "{} exists; hub installed its own unit",
        installed.display()
    );
    assert!(
        !home.join("config/systemd").exists(),
        "hub created a systemd directory"
    );
    // And nothing outside the two places §4.2 names.
    assert!(home.join("config/hub").is_dir(), "its config directory");
}

#[test]
fn the_unit_names_no_working_directory_which_is_why_ac8_exists() {
    // `WorkingDirectory` defaults to %h, and $HOME is not a checkout. That is
    // the whole reason recent activity is a per-project fan-out.
    assert!(!unit().contains("WorkingDirectory"), "{}", unit());
}

#[test]
fn subscribe_prints_the_topic_both_links_and_what_it_costs() {
    let dir = TempDir::new("subscribe");
    let home = dir.join("home");
    let config = dir.join("config.toml");
    std::fs::write(
        &config,
        "topic = \"workflow-0123456789ABCDEFGHJKMNPQ\"\nntfy_base = \"http://127.0.0.1:9\"\n",
    )
    .unwrap();
    let hub = Hub::spawn(
        &home,
        &[],
        &["--config", config.to_str().unwrap(), "--port", "0"],
    );

    let response = hub.get("/subscribe");
    assert_eq!(status_of(&response), 200);
    let body = body_of(&response);

    assert!(body.contains("workflow-0123456789ABCDEFGHJKMNPQ"), "{body}");
    assert!(
        body.contains("ntfy://workflow-0123456789ABCDEFGHJKMNPQ"),
        "{body}"
    );
    assert!(
        body.contains("http://127.0.0.1:9/workflow-0123456789ABCDEFGHJKMNPQ"),
        "{body}"
    );
    // m-1: the accurate sentence, not "content stays on the tailnet".
    assert!(body.contains("never leaves the tailnet"), "{body}");
    assert!(
        body.contains("anyone holding it can also publish"),
        "{body}"
    );
    // No QR, and therefore no image crate.
    assert!(!body.contains("<img"), "{body}");
    assert!(!body.contains("<script"), "{body}");
}

#[test]
fn a_topic_with_html_in_it_is_still_escaped_on_the_subscribe_page() {
    // The topic is generated, so this cannot happen by accident — but the page
    // that prints the one secret hub holds should not be the one place where
    // escaping was skipped.
    let dir = TempDir::new("subscribe-esc");
    let home = dir.join("home");
    let config = dir.join("config.toml");
    std::fs::write(
        &config,
        "topic = \"<script>alert(1)</script>\"\nntfy_base = \"http://127.0.0.1:9\"\n",
    )
    .unwrap();
    let hub = Hub::spawn(
        &home,
        &[],
        &["--config", config.to_str().unwrap(), "--port", "0"],
    );

    let body = body_of(&hub.get("/subscribe")).to_string();
    assert!(!body.contains("<script>alert(1)"), "{body}");
    assert!(body.contains("&lt;script&gt;"), "{body}");
}

#[test]
fn the_config_hub_writes_on_first_run_is_the_shape_the_unit_expects() {
    // The unit sets no XDG variables, so hub finds its config at
    // ~/.config/hub/config.toml — and creates it, 0600, on first start.
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new("first-run");
    let home = dir.join("home");
    let hub = Hub::spawn(&home, &[], &["--port", "0"]);
    assert_eq!(status_of(&hub.get("/")), 200);

    // Hub::spawn writes a 0600 test config first so nothing can publish to
    // ntfy.sh; the point here is that the location is the one the unit will
    // meet, and that hub left the mode alone.
    let path = home.join("config/hub/config.toml");
    assert!(path.is_file(), "{}", path.display());
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("topic ="), "{text}");
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "hub loosened the mode of its own config");
}
