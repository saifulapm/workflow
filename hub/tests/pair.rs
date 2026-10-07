//! `hub pair` and `/pair/<code>`: the link the command prints, the page that
//! link opens, and the cookie its Pair button hands the device.

mod common;

use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::Duration;

use common::{Hub, TempDir, body_of, fixture_mem, header_of, hub_bin, status_of, wait_for};

/// Written to the machine file hub reads, so the link's host is a name no
/// real machine has.
const MACHINE: &str = "pair-box";
const IPHONE: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) \
                      AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148 Safari/604.1";

/// A home with a machine name and a config, and a `mem` that knows no
/// projects: pairing never asks mem anything.
struct World {
    _dir: TempDir,
    home: PathBuf,
    bin: PathBuf,
    config: PathBuf,
}

impl World {
    fn new(tag: &str) -> World {
        let dir = TempDir::new(tag);
        let home = dir.join("home");
        std::fs::create_dir_all(home.join("config/qshell")).unwrap();
        std::fs::write(home.join("config/qshell/machine"), MACHINE).unwrap();
        let bin = dir.join("bin");
        fixture_mem(&bin, "echo '{\"projects\":[]}'");
        let config = dir.join("hub.toml");
        std::fs::write(
            &config,
            "topic = \"workflow-TESTTESTTESTTESTTESTTESTTE\"\n\
             ntfy_base = \"http://127.0.0.1:9\"\n",
        )
        .unwrap();
        World {
            _dir: dir,
            home,
            bin,
            config,
        }
    }

    fn hub(&self) -> Hub {
        Hub::spawn(
            &self.home,
            &[&self.bin],
            &["--config", self.config.to_str().unwrap(), "--port", "0"],
        )
    }

    /// `hub pair` with the environment `Hub::spawn` gives a hub, so the two
    /// share one state directory.
    fn pair(&self) -> Pairing {
        let mut child = Command::new(hub_bin())
            .args(["pair", "--config", self.config.to_str().unwrap()])
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("XDG_STATE_HOME", self.home.join("state"))
            .env("XDG_DATA_HOME", self.home.join("data"))
            .env("XDG_CACHE_HOME", self.home.join("cache"))
            .env("HUB_TAILNET_NAME", "")
            .env("HUB_PAIR_POLL_MS", "20")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn hub pair");
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let mut first = String::new();
        let mut link = String::new();
        stdout.read_line(&mut first).unwrap();
        stdout.read_line(&mut link).unwrap();
        assert_eq!(first, "Open on the device, within 10 minutes:\n");
        let link = link.trim().to_string();
        Pairing {
            child,
            stdout,
            link,
        }
    }

    fn state(&self, name: &str) -> PathBuf {
        self.home.join("state/hub").join(name)
    }
}

struct Pairing {
    child: Child,
    stdout: BufReader<ChildStdout>,
    link: String,
}

impl Pairing {
    /// The path the link opens, `/pair/<code>`.
    fn path(&self) -> String {
        let prefix = format!("http://{MACHINE}:8787");
        self.link
            .strip_prefix(&prefix)
            .unwrap_or_else(|| panic!("{:?} does not start with {prefix}", self.link))
            .to_string()
    }

    /// Waits for the command to end, and returns its exit code and the rest of
    /// what it printed.
    fn finish(mut self) -> (Option<i32>, String) {
        let mut status = None;
        wait_for("hub pair to exit", Duration::from_secs(10), || {
            status = self.child.try_wait().unwrap();
            status.is_some()
        });
        let mut rest = String::new();
        self.stdout.read_to_string(&mut rest).unwrap();
        (status.unwrap().code(), rest)
    }
}

impl Drop for Pairing {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn the_printed_link_pairs_the_device_that_presses_pair() {
    let world = World::new("pair-flow");
    let pairing = world.pair();
    let path = pairing.path();
    let code = path.strip_prefix("/pair/").expect("a /pair/ link");
    assert_eq!(code.len(), 6, "{path}");
    assert!(
        code.bytes()
            .all(|b| b"0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(&b)),
        "{code}"
    );

    let hub = world.hub();
    let page = hub.get(&path);
    assert_eq!(status_of(&page), 200, "{page}");
    let body = body_of(&page);
    assert!(
        body.contains(&format!("<form method=\"post\" action=\"/pair/{code}\">")),
        "{body}"
    );
    assert!(body.contains("Pair this device</button>"), "{body}");

    let response = hub.post_form_with(
        &path,
        "",
        &[("Origin", &hub.origin()), ("User-Agent", IPHONE)],
    );
    assert_eq!(status_of(&response), 303, "{response}");
    assert_eq!(header_of(&response, "location"), Some("/"));
    let cookie = header_of(&response, "set-cookie").expect("a Set-Cookie");
    let token = cookie
        .strip_prefix("hub_device=")
        .and_then(|rest| rest.strip_suffix("; Path=/; Max-Age=31536000; HttpOnly; SameSite=Strict"))
        .unwrap_or_else(|| panic!("{cookie}"));
    assert_eq!(token.len(), 26, "{cookie}");

    let devices = std::fs::read_to_string(world.state("devices")).unwrap();
    let line = devices.lines().last().unwrap();
    assert!(line.starts_with(&format!("{token} ")), "{devices}");
    assert!(line.ends_with(" iphone"), "{devices}");

    let (code, rest) = pairing.finish();
    assert_eq!(rest, "✓ paired: iphone (cookie for 1 year)\n");
    assert_eq!(code, Some(0));
}

#[test]
fn a_code_pairs_once_and_not_after_it_expires() {
    let world = World::new("pair-once");
    let pairing = world.pair();
    let path = pairing.path();
    let hub = world.hub();

    let first = hub.post_form(&path, "");
    assert_eq!(status_of(&first), 303, "{first}");
    let again = hub.post_form(&path, "");
    assert_eq!(status_of(&again), 404, "{again}");
    assert_eq!(header_of(&again, "set-cookie"), None, "{again}");
    assert_eq!(status_of(&hub.get(&path)), 404);

    // One code long past its expiry and one far ahead of it, written the way
    // `hub pair` writes them.
    std::fs::write(world.state("pair"), "XPRD42 1000\nFRSH42 99999999999999\n").unwrap();
    assert_eq!(status_of(&hub.get("/pair/FRSH42")), 200);
    let get = hub.get("/pair/XPRD42");
    assert_eq!(status_of(&get), 404, "{get}");
    assert!(body_of(&get).contains("hub pair"), "{get}");
    let post = hub.post_form("/pair/XPRD42", "");
    assert_eq!(status_of(&post), 404, "{post}");
    assert_eq!(header_of(&post, "set-cookie"), None, "{post}");
}

#[test]
fn the_pair_and_devices_files_are_private() {
    let world = World::new("pair-modes");
    let pairing = world.pair();
    assert_eq!(mode(&world.state("pair")), 0o600);
    let hub = world.hub();
    let response = hub.post_form(&pairing.path(), "");
    assert_eq!(status_of(&response), 303, "{response}");
    assert_eq!(mode(&world.state("pair")), 0o600);
    assert_eq!(mode(&world.state("devices")), 0o600);
}

#[test]
fn a_cross_origin_post_leaves_the_code_pending() {
    let world = World::new("pair-origin");
    let pairing = world.pair();
    let path = pairing.path();
    let hub = world.hub();
    let response = hub.post_form_with(&path, "", &[("Origin", "https://evil.example")]);
    assert_eq!(status_of(&response), 403, "{response}");
    assert_eq!(header_of(&response, "set-cookie"), None, "{response}");
    assert_eq!(status_of(&hub.get(&path)), 200);
    let code = path.strip_prefix("/pair/").unwrap();
    let pending = std::fs::read_to_string(world.state("pair")).unwrap();
    assert!(pending.starts_with(&format!("{code} ")), "{pending}");
}
