//! mem's remote mode end to end: a client `mem` with MEM_REMOTE set and an
//! empty store of its own runs every verb on the hub's store, through the
//! MCP listener's `POST /cli`, behind the same Access check as `/mcp`.
//!
//! The Access keys and the fixture `curl` that serves their JWKS to the hub
//! are the ones `tests/mcp.rs` uses. The client gets the real curl.

mod common;

use std::io::{BufRead, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::LazyLock;
use std::time::Duration;

use common::{Hub, TempDir, fixture_bin, invocations, mem_build, mem_in, recording_mem};
use rsa::rand_core::{Infallible, TryCryptoRng, TryRng};
use rsa::traits::PublicKeyParts;
use rsa::{Pkcs1v15Sign, RsaPrivateKey};
use serde_json::json;
use sha2::{Digest, Sha256};

const TEAM: &str = "https://test-team.cloudflareaccess.com";
const AUD: &str = "4714c1358e65fe4b408ad6d432a5f878f08194bdb4752441fd56faefa9b2b6f2";
const KID: &str = "test-kid-1";
const BINARY: &str = "#!/bin/sh\necho a stand-in for mem\n";
const HUB_BINARY: &str = "#!/bin/sh\necho a stand-in for hub\n";

struct Urandom(std::fs::File);

impl TryRng for Urandom {
    type Error = Infallible;
    fn try_next_u32(&mut self) -> Result<u32, Infallible> {
        let mut b = [0u8; 4];
        self.try_fill_bytes(&mut b)?;
        Ok(u32::from_le_bytes(b))
    }
    fn try_next_u64(&mut self) -> Result<u64, Infallible> {
        let mut b = [0u8; 8];
        self.try_fill_bytes(&mut b)?;
        Ok(u64::from_le_bytes(b))
    }
    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Infallible> {
        self.0.read_exact(dst).expect("read /dev/urandom");
        Ok(())
    }
}

impl TryCryptoRng for Urandom {}

static KEY: LazyLock<RsaPrivateKey> = LazyLock::new(|| {
    let mut rng = Urandom(std::fs::File::open("/dev/urandom").unwrap());
    RsaPrivateKey::new(&mut rng, 2048).unwrap()
});

/// This checkout's mem, rebuilt once per run: `real_mem` reuses whatever
/// binary is already there, and these tests drive the client side of it too.
static MEM: LazyLock<PathBuf> = LazyLock::new(|| {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    assert!(mem_build(root).status().unwrap().success(), "build mem");
    root.join("mem/target/release/mem")
});

fn b64url(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().fold(0u32, |n, b| n << 8 | u32::from(*b)) << (8 * (3 - chunk.len()));
        for i in 0..=chunk.len() {
            out.push(ABC[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

fn token() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let header = json!({"alg": "RS256", "kid": KID, "typ": "JWT"});
    let claims = json!({
        "aud": [AUD], "email": "saiful@example.com", "exp": now + 300, "iat": now,
        "nbf": now, "iss": TEAM, "type": "app",
    });
    let input = format!(
        "{}.{}",
        b64url(header.to_string().as_bytes()),
        b64url(claims.to_string().as_bytes())
    );
    let digest = Sha256::digest(input.as_bytes());
    let signature = KEY.sign(Pkcs1v15Sign::new::<Sha256>(), &digest).unwrap();
    format!("{input}.{}", b64url(&signature))
}

fn git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("run git");
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

/// A checkout named `gamma` whose origin is github.com/me/gamma.
fn checkout(at: &Path) -> PathBuf {
    let repo = at.join("gamma");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(
        &repo,
        &["remote", "add", "origin", "git@github.com:me/gamma.git"],
    );
    repo
}

struct World {
    _dir: TempDir,
    /// The hub's home, which holds the server store.
    home: PathBuf,
    /// The client's home: its own XDG dirs, never written.
    client: PathBuf,
    /// The client's checkout of gamma.
    repo: PathBuf,
    mem_log: PathBuf,
    _hub: Hub,
    mcp_port: u16,
}

impl World {
    fn new(tag: &str) -> World {
        let dir = TempDir::new(tag);
        let home = dir.join("home");
        let client = dir.join("client");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&client).unwrap();
        let mem = &*MEM;
        let (bin, mem_log) = recording_mem(dir.path(), &home);

        // The server store holds gamma, registered from a checkout whose
        // origin is github.com/me/gamma.
        let server_repo = checkout(&home);
        let out = mem_in(mem, &home, &server_repo, &["log", "gamma begins"]);
        assert!(out.status.success(), "{out:?}");

        let public = KEY.to_public_key();
        let jwks = json!({
            "keys": [{
                "kid": KID, "alg": "RS256", "kty": "RSA", "use": "sig",
                "e": b64url(&public.e().to_be_bytes_trimmed_vartime()),
                "n": b64url(&public.n().to_be_bytes_trimmed_vartime()),
            }],
            "public_certs": [],
        });
        let jwks_path = dir.join("jwks.json");
        std::fs::write(&jwks_path, jwks.to_string()).unwrap();
        fixture_bin(
            &bin,
            "curl",
            &format!(
                "case \"$*\" in *'{TEAM}/cdn-cgi/access/certs'*) cat '{}';; esac\nexit 0",
                jwks_path.display()
            ),
        );

        let binary = dir.join("mem-binary");
        std::fs::write(&binary, BINARY).unwrap();
        let hub_binary = dir.join("hub-binary");
        std::fs::write(&hub_binary, HUB_BINARY).unwrap();
        let config = dir.join("config.toml");
        std::fs::write(
            &config,
            format!(
                "topic = \"workflow-TESTTESTTESTTESTTESTTESTTE\"\n\
                 ntfy_base = \"http://127.0.0.1:9\"\n\
                 mcp_port = 0\naccess_aud = \"{AUD}\"\naccess_team = \"{TEAM}\"\n\
                 mem_binary = \"{}\"\nhub_binary = \"{}\"\n",
                binary.display(),
                hub_binary.display()
            ),
        )
        .unwrap();
        let mut hub = Hub::spawn(
            &home,
            &[&bin],
            &["--config", config.to_str().unwrap(), "--port", "0"],
        );
        let mut line = String::new();
        hub.stdout.read_line(&mut line).expect("mcp start line");
        let mcp_port = line
            .trim()
            .strip_prefix("mcp listening on 127.0.0.1:")
            .and_then(|p| p.parse().ok())
            .unwrap_or_else(|| panic!("no mcp port in {line:?}"));

        let repo = checkout(&client);
        World {
            _dir: dir,
            home,
            client,
            repo,
            mem_log,
            _hub: hub,
            mcp_port,
        }
    }

    /// The client `mem`, its stdin `input`, with the Access header when
    /// `authed`.
    fn client(&self, args: &[&str], input: &str, authed: bool) -> Output {
        let mut command = Command::new(&*MEM);
        command
            .args(args)
            .current_dir(&self.repo)
            .env_clear()
            .env("HOME", &self.client)
            .env("PATH", "/usr/bin:/bin")
            .env("XDG_CONFIG_HOME", self.client.join("config"))
            .env("XDG_STATE_HOME", self.client.join("state"))
            .env("XDG_DATA_HOME", self.client.join("data"))
            .env("XDG_CACHE_HOME", self.client.join("cache"))
            .env("MEM_SYNC_CMD", "true")
            .env("MEM_NOTIFY_CMD", "true")
            .env("MEM_REMOTE", format!("http://127.0.0.1:{}/", self.mcp_port))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if authed {
            command.env(
                "MEM_REMOTE_HEADER",
                format!("Cf-Access-Jwt-Assertion: {}", token()),
            );
        }
        let mut child = command.spawn().expect("spawn client mem");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    /// mem run straight on the server store.
    fn server(&self, args: &[&str]) -> String {
        let out = mem_in(&MEM, &self.home, &self.home, args);
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn ran(&self, verb: &str) -> bool {
        invocations(&self.mem_log)
            .iter()
            .any(|argv| argv.iter().any(|a| a == verb))
    }

    fn get(&self, path: &str, token: Option<&str>) -> Vec<u8> {
        let auth = token
            .map(|t| format!("Cf-Access-Jwt-Assertion: {t}\r\n"))
            .unwrap_or_default();
        let mut stream = TcpStream::connect(("127.0.0.1", self.mcp_port)).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        stream
            .write_all(
                format!(
                    "GET {path} HTTP/1.1\r\nHost: mem.on-rexa.com\r\nConnection: close\r\n\
                     {auth}\r\n"
                )
                .as_bytes(),
            )
            .unwrap();
        let mut out = Vec::new();
        let _ = stream.read_to_end(&mut out);
        out
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Every file under `dir`, so "nothing written" means nothing at all.
fn files_under(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(files_under(&path));
        } else {
            out.push(path);
        }
    }
    out
}

#[test]
fn a_log_through_the_client_lands_in_the_server_store_and_nowhere_else() {
    let world = World::new("remote-log");
    let out = world.client(&["log", "x"], "", true);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));

    let lines = world.server(&["--project", "gamma", "log"]);
    assert!(lines.contains(" x"), "{lines}");
    assert!(lines.contains("gamma begins"), "{lines}");
    assert_eq!(
        files_under(&world.client.join("data")),
        Vec::<PathBuf>::new()
    );
    assert_eq!(
        files_under(&world.client.join("cache")),
        Vec::<PathBuf>::new()
    );
}

#[test]
fn a_handoff_and_a_plan_from_stdin_round_trip_through_the_server() {
    let world = World::new("remote-handoff");
    let out = world.client(&["handoff", "--set", "parked: t2"], "", true);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let out = world.client(&["handoff"], "", true);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(
        text(&out.stdout).contains("parked: t2"),
        "{}",
        text(&out.stdout)
    );

    let plan = "# plan: m8\n- [ ] t1 remote mode\n- [ ] t2 the server\n";
    let out = world.client(&["plan", "--stdin"], plan, true);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(world.server(&["--project", "gamma", "plan"]), plan);
}

#[test]
fn a_verb_off_the_allowlist_is_refused_and_never_runs() {
    let world = World::new("remote-prune");
    let out = world.client(&["prune"], "", true);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));
    let stderr = text(&out.stderr);
    assert!(
        stderr.contains("prune") && stderr.contains("refused"),
        "{stderr}"
    );
    assert!(!world.ran("prune"), "{:?}", invocations(&world.mem_log));
}

#[test]
fn without_the_access_header_the_client_reports_the_401() {
    let world = World::new("remote-401");
    let out = world.client(&["log", "x"], "", false);
    assert_ne!(out.status.code(), Some(0));
    let stderr = text(&out.stderr);
    assert!(stderr.contains("answered 401"), "{stderr}");
    assert!(!world.ran("x"));
}

#[test]
fn the_mem_binary_downloads_only_with_a_token() {
    let world = World::new("remote-dl");
    let response = world.get("/dl/mem", Some(&token()));
    let response = text(&response);
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.contains("application/octet-stream"), "{response}");
    assert!(
        response.ends_with(&format!("\r\n\r\n{BINARY}")),
        "{response}"
    );

    let response = text(&world.get("/dl/mem", None));
    assert!(response.starts_with("HTTP/1.1 401"), "{response}");
}

/// A cloud session runs its local hub from the binary the mem server hands it.
#[test]
fn the_hub_binary_downloads_only_with_a_token() {
    let world = World::new("remote-dl-hub");
    let response = text(&world.get("/dl/hub", Some(&token())));
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(
        response.ends_with(&format!("\r\n\r\n{HUB_BINARY}")),
        "{response}"
    );

    let response = text(&world.get("/dl/hub", None));
    assert!(response.starts_with("HTTP/1.1 401"), "{response}");
}
