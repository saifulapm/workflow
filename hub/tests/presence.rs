//! Presence (`/api/presence`) — the doorbell's routing question.
//!
//! Presence is positive evidence only: the shell process alive, the qshell
//! state directory present, no lock marker. A machine with no shell at all —
//! the nuc, or a dead session that left a stale `locked` marker behind — is
//! never "watching".

mod common;

use std::path::{Path, PathBuf};

use common::{Hub, TempDir, fixture_bin};

struct World {
    home: PathBuf,
    bin: PathBuf,
    config: PathBuf,
    _dir: TempDir,
}

impl World {
    fn new(tag: &str) -> World {
        let dir = TempDir::new(tag);
        let home = dir.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let config = dir.join("config.toml");
        std::fs::write(
            &config,
            "topic = \"workflow-TESTTESTTESTTESTTESTTESTTE\"\nntfy_base = \"http://127.0.0.1:9\"\n",
        )
        .unwrap();
        World {
            home,
            bin,
            config,
            _dir: dir,
        }
    }

    fn hub(&self) -> Hub {
        Hub::spawn_env(
            &self.home,
            &[&self.bin],
            &["--config", self.config.to_str().unwrap(), "--port", "0"],
            &[("HUB_POLL_MS", "120")],
        )
    }

    fn qshell_dir(&self) -> PathBuf {
        self.home.join("state/qshell")
    }

    fn flag(&self, name: &str) {
        std::fs::create_dir_all(self.qshell_dir()).unwrap();
        std::fs::write(self.qshell_dir().join(name), "").unwrap();
    }

    fn unflag(&self, name: &str) {
        let _ = std::fs::remove_file(self.qshell_dir().join(name));
    }

    fn shell(&self, alive: bool) {
        fixture_bin(&self.bin, "pgrep", if alive { "exit 0" } else { "exit 1" });
    }
}

fn field(response: &str, json: &str) -> bool {
    response.contains(json)
}

#[test]
fn no_qshell_state_directory_means_nobody_is_watching() {
    // The nuc's shape: no shell has ever run here. A stale pgrep answer must
    // not matter, so the fixture says the process exists.
    let world = World::new("presence-headless");
    world.shell(true);
    let hub = world.hub();
    let response = hub.get("/api/presence");
    assert!(field(&response, "\"watching\":false"), "{response}");
}

#[test]
fn a_live_unlocked_shell_is_watching_and_the_flags_turn_it_off() {
    let world = World::new("presence-flags");
    std::fs::create_dir_all(world.qshell_dir()).unwrap();
    world.shell(true);
    let hub = world.hub();

    assert!(
        field(&hub.get("/api/presence"), "\"watching\":true"),
        "unlocked live shell should be watching"
    );

    world.flag("locked");
    assert!(
        field(&hub.get("/api/presence"), "\"watching\":false"),
        "a lock marker means not watching"
    );
    world.unflag("locked");

    world.flag("idle");
    assert!(
        field(&hub.get("/api/presence"), "\"watching\":false"),
        "an active idle cycle means not watching"
    );

    // Stay-awake overrides idle: Saiful parked at the machine watching
    // something long is the most-watching state there is.
    world.flag("stay-awake");
    assert!(
        field(&hub.get("/api/presence"), "\"watching\":true"),
        "stay-awake overrides the idle flag"
    );
}

#[test]
fn a_stale_lock_marker_with_no_shell_process_is_not_watching() {
    let world = World::new("presence-stale");
    std::fs::create_dir_all(world.qshell_dir()).unwrap();
    world.shell(false);
    let hub = world.hub();
    let response = hub.get("/api/presence");
    assert!(field(&response, "\"watching\":false"), "{response}");
    assert!(field(&response, "\"shell\":false"), "{response}");
}

/// Keeps rustc from flagging helpers only some tests use.
#[allow(dead_code)]
fn keep(_: &Path) {}
