//! mem's MCP endpoint: a second loopback port that answers JSON-RPC over
//! POST, and only to a request carrying a Cloudflare Access token that this
//! hub's Access app signed for it.
//!
//! The tests make their own RSA keys, serve one as the team's JWKS through a
//! fixture `curl`, and sign tokens with it (or with a stranger's key).

mod common;

use std::io::{BufRead, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::Duration;

use common::{
    Hub, TempDir, body_of, fixture_bin, header_of, invocations, mem_in, real_mem, recording_mem,
    seed_project, status_of,
};
use rsa::rand_core::{Infallible, TryCryptoRng, TryRng};
use rsa::traits::PublicKeyParts;
use rsa::{Pkcs1v15Sign, RsaPrivateKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const TEAM: &str = "https://test-team.cloudflareaccess.com";
const AUD: &str = "4714c1358e65fe4b408ad6d432a5f878f08194bdb4752441fd56faefa9b2b6f2";
const OTHER_AUD: &str = "0d5f7e5c1a0b9c8d7e6f5a4b3c2d1e0f9a8b7c6d5e4f3a2b1c0d9e8f7a6b5c4d";
const KID: &str = "test-kid-1";
const HANDOFF: &str = "Next: wire the connector to claude.ai.";

/// `/dev/urandom` as the keygen's RNG, so the tests need no RNG crate.
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

/// The team's signing key and a key the team never published.
static KEYS: LazyLock<(RsaPrivateKey, RsaPrivateKey)> = LazyLock::new(|| {
    let mut rng = Urandom(std::fs::File::open("/dev/urandom").unwrap());
    (
        RsaPrivateKey::new(&mut rng, 2048).unwrap(),
        RsaPrivateKey::new(&mut rng, 2048).unwrap(),
    )
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

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// The claims Access puts in a token for this app, valid for five minutes.
fn claims() -> Value {
    json!({
        "aud": [AUD],
        "email": "saiful@example.com",
        "exp": now() + 300,
        "iat": now(),
        "nbf": now(),
        "iss": TEAM,
        "type": "app",
        "sub": "7335d417-61da-459d-899c-0a01c76a2f94",
    })
}

fn sign(key: &RsaPrivateKey, header: &Value, claims: &Value) -> String {
    let input = format!(
        "{}.{}",
        b64url(header.to_string().as_bytes()),
        b64url(claims.to_string().as_bytes())
    );
    let digest = Sha256::digest(input.as_bytes());
    let signature = key.sign(Pkcs1v15Sign::new::<Sha256>(), &digest).unwrap();
    format!("{input}.{}", b64url(&signature))
}

fn token_with(claims: &Value) -> String {
    sign(
        &KEYS.0,
        &json!({"alg": "RS256", "kid": KID, "typ": "JWT"}),
        claims,
    )
}

fn token() -> String {
    token_with(&claims())
}

struct World {
    _dir: TempDir,
    home: PathBuf,
    mem: PathBuf,
    mem_log: PathBuf,
    hub: Hub,
    mcp_port: u16,
}

impl World {
    fn new(tag: &str) -> World {
        let dir = TempDir::new(tag);
        let home = dir.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let mem = real_mem().expect("build mem first");
        let (bin, mem_log) = recording_mem(dir.path(), &home);
        seed_project(&mem, &home, "proj-alpha", "alpha did a thing");
        let out = mem_in(
            &mem,
            &home,
            &home.join("proj-alpha"),
            &["handoff", "--set", HANDOFF],
        );
        assert!(out.status.success(), "{out:?}");

        let public = KEYS.0.to_public_key();
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

        let config = dir.join("config.toml");
        std::fs::write(
            &config,
            format!(
                "topic = \"workflow-TESTTESTTESTTESTTESTTESTTE\"\n\
                 ntfy_base = \"http://127.0.0.1:9\"\n\
                 mcp_port = 0\naccess_aud = \"{AUD}\"\naccess_team = \"{TEAM}\"\n"
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
        World {
            _dir: dir,
            home,
            mem,
            mem_log,
            hub,
            mcp_port,
        }
    }

    fn raw(&self, request: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", self.mcp_port)).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut out = Vec::new();
        let _ = stream.read_to_end(&mut out);
        String::from_utf8_lossy(&out).into_owned()
    }

    /// A JSON-RPC POST, the shape claude.ai sends through the tunnel.
    fn post(&self, token: Option<&str>, message: &Value) -> String {
        let body = message.to_string();
        let auth = token
            .map(|t| format!("Cf-Access-Jwt-Assertion: {t}\r\n"))
            .unwrap_or_default();
        self.raw(&format!(
            "POST /mcp HTTP/1.1\r\nHost: mem.on-rexa.com\r\nConnection: close\r\n\
             Content-Type: application/json\r\nAccept: application/json, text/event-stream\r\n\
             {auth}Content-Length: {}\r\n\r\n{body}",
            body.len()
        ))
    }

    fn call(&self, token: Option<&str>, method: &str, params: Value) -> String {
        self.post(
            token,
            &json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}),
        )
    }

    fn handoff_call(&self, token: Option<&str>) -> String {
        self.call(
            token,
            "tools/call",
            json!({"name": "handoff", "arguments": {"project": "proj-alpha"}}),
        )
    }

    fn handoff_runs(&self) -> usize {
        invocations(&self.mem_log)
            .iter()
            .filter(|argv| argv.iter().any(|a| a == "handoff"))
            .count()
    }
}

fn result_of(response: &str) -> Value {
    assert_eq!(status_of(response), 200, "{response}");
    let doc: Value =
        serde_json::from_str(body_of(response)).unwrap_or_else(|_| panic!("{response}"));
    doc["result"].clone()
}

#[test]
fn a_request_without_an_access_token_is_refused_before_mem_runs() {
    let world = World::new("mcp-none");
    let response = world.handoff_call(None);
    assert_eq!(status_of(&response), 401, "{response}");
    assert_eq!(world.handoff_runs(), 0);
}

#[test]
fn a_token_signed_by_a_key_the_team_never_published_is_refused() {
    let world = World::new("mcp-stranger");
    let header = json!({"alg": "RS256", "kid": KID, "typ": "JWT"});
    let forged = sign(&KEYS.1, &header, &claims());
    assert_eq!(status_of(&world.handoff_call(Some(&forged))), 401);
    assert_eq!(world.handoff_runs(), 0);
    // The same claims under the team's key pass, so the 401 was the key.
    assert_eq!(status_of(&world.handoff_call(Some(&token()))), 200);
}

#[test]
fn a_token_issued_for_another_access_app_is_refused() {
    let world = World::new("mcp-aud");
    let mut other = claims();
    other["aud"] = json!([OTHER_AUD]);
    assert_eq!(
        status_of(&world.handoff_call(Some(&token_with(&other)))),
        401
    );
    other["aud"] = json!(OTHER_AUD);
    assert_eq!(
        status_of(&world.handoff_call(Some(&token_with(&other)))),
        401
    );
    assert_eq!(world.handoff_runs(), 0);
    // A bare string aud naming this app is as good as the array.
    let mut ours = claims();
    ours["aud"] = json!(AUD);
    assert_eq!(
        status_of(&world.handoff_call(Some(&token_with(&ours)))),
        200
    );
}

#[test]
fn an_expired_token_and_an_unsigned_one_are_refused() {
    let world = World::new("mcp-expired");
    let mut expired = claims();
    expired["exp"] = json!(now() - 60);
    expired["iat"] = json!(now() - 600);
    expired["nbf"] = json!(now() - 600);
    assert_eq!(
        status_of(&world.handoff_call(Some(&token_with(&expired)))),
        401
    );

    let header = json!({"alg": "none", "kid": KID, "typ": "JWT"});
    let unsigned = format!(
        "{}.{}.",
        b64url(header.to_string().as_bytes()),
        b64url(claims().to_string().as_bytes())
    );
    assert_eq!(status_of(&world.handoff_call(Some(&unsigned))), 401);
    // The alg is refused for itself, not only for the missing signature.
    let labelled = sign(&KEYS.0, &header, &claims());
    assert_eq!(status_of(&world.handoff_call(Some(&labelled))), 401);
    assert_eq!(world.handoff_runs(), 0);
}

#[test]
fn a_valid_token_initializes_lists_handoff_and_quotes_it() {
    let world = World::new("mcp-valid");
    let token = token();

    let init = result_of(&world.call(
        Some(&token),
        "initialize",
        json!({
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {"name": "claude-ai", "version": "1.0"},
        }),
    ));
    assert_eq!(init["serverInfo"]["name"], "mem", "{init}");
    assert_eq!(init["protocolVersion"], "2025-11-25", "{init}");

    let listed = result_of(&world.call(Some(&token), "tools/list", json!({})));
    let names: Vec<&str> = listed["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("{listed}"))
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["handoff"]);

    let called = result_of(&world.handoff_call(Some(&token)));
    let printed = mem_in(
        &world.mem,
        &world.home,
        &world.home,
        &["handoff", "--project=proj-alpha"],
    );
    let printed = String::from_utf8(printed.stdout).unwrap();
    assert!(printed.contains(HANDOFF), "{printed}");
    assert_eq!(called["isError"], json!(false), "{called}");
    assert_eq!(called["content"][0]["type"], "text", "{called}");
    assert_eq!(called["content"][0]["text"], printed.as_str(), "{called}");
}

#[test]
fn the_hub_pages_stay_on_their_port_and_off_the_mcp_one() {
    let world = World::new("mcp-ports");
    let page = world.hub.get("/");
    assert_eq!(status_of(&page), 200, "{page}");
    assert!(body_of(&page).contains("proj-alpha"), "{page}");

    let response = world.raw(&format!(
        "GET / HTTP/1.1\r\nHost: mem.on-rexa.com\r\nConnection: close\r\n\
         Cf-Access-Jwt-Assertion: {}\r\n\r\n",
        token()
    ));
    assert_eq!(status_of(&response), 405, "{response}");
    assert_eq!(header_of(&response, "allow"), Some("POST"));
}
