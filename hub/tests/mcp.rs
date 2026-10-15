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
/// Every tool the connector lists: the reads, then an orchestrator's writes.
const TOOL_NAMES: [&str; 20] = [
    "context",
    "search",
    "show",
    "wiki",
    "questions",
    "plan",
    "roadmap",
    "log",
    "handoff",
    "ask",
    "answer",
    "set_handoff",
    "add_log",
    "decide",
    "tick_task",
    "tick_milestone",
    "write_wiki",
    "add_evidence",
    "add_finding",
    "close_finding",
];

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
    assert_eq!(names, TOOL_NAMES);

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

impl World {
    /// A `tools/call` of `name` with a valid token, its result.
    fn tool(&self, name: &str, arguments: Value) -> Value {
        result_of(&self.call(
            Some(&token()),
            "tools/call",
            json!({"name": name, "arguments": arguments}),
        ))
    }

    /// mem runs other than the hub's own background reads, which all ask
    /// for `--json` while no connector tool ever does.
    fn tool_runs(&self) -> usize {
        invocations(&self.mem_log)
            .iter()
            .filter(|argv| !argv.iter().any(|a| a == "--json"))
            .count()
    }

    /// What the real mem prints for `args`, run straight against the store.
    fn printed(&self, args: &[&str]) -> String {
        let out = mem_in(&self.mem, &self.home, &self.home, args);
        String::from_utf8(out.stdout).unwrap()
    }

    /// A plan with two tasks, a roadmap with two milestones and a wiki page,
    /// written the way an orchestrator writes them, outside the connector.
    fn seed_plan_roadmap_wiki(&self) {
        let plan = self.home.join("plan.md");
        std::fs::write(
            &plan,
            "# plan: m1-auth\n\n- [ ] t1 Sign in\n- [ ] t2 Sign out\n",
        )
        .unwrap();
        let roadmap = self.home.join("roadmap.md");
        std::fs::write(
            &roadmap,
            "# roadmap: alpha\n\n- [ ] m1-auth Sign-in\n- [ ] m2-billing Billing\n",
        )
        .unwrap();
        for args in [
            vec![
                "--project=proj-alpha",
                "plan",
                "--set-file",
                plan.to_str().unwrap(),
            ],
            vec![
                "--project=proj-alpha",
                "roadmap",
                "--set-file",
                roadmap.to_str().unwrap(),
            ],
            vec!["--project=proj-alpha", "roadmap", "--status", "draft"],
            vec![
                "--project=proj-alpha",
                "ask",
                "--for=human",
                "--",
                "Ship it?",
            ],
        ] {
            let out = mem_in(&self.mem, &self.home, &self.home, &args);
            assert!(out.status.success(), "{args:?}: {out:?}");
        }
        let mut child = std::process::Command::new(&self.mem)
            .args([
                "--project=proj-alpha",
                "wiki",
                "--stdin",
                "--note=seed",
                "--",
                "auth",
            ])
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", "/usr/bin:/bin")
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("XDG_STATE_HOME", self.home.join("state"))
            .env("XDG_DATA_HOME", self.home.join("data"))
            .env("XDG_CACHE_HOME", self.home.join("cache"))
            .env("MEM_SYNC_CMD", "true")
            .env("MEM_NOTIFY_CMD", "true")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"# Auth\n\nSign in with a PIN.\n\n## Tokens\n\nOne per app.\n")
            .unwrap();
        assert!(child.wait().unwrap().success());
    }
}

fn text_of(result: &Value) -> &str {
    result["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("{result}"))
}

#[test]
fn the_tool_list_offers_nothing_that_deletes_prunes_clears_replaces_or_approves() {
    let world = World::new("mcp-list");
    let listed = result_of(&world.call(Some(&token()), "tools/list", json!({})));
    let tools = listed["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, TOOL_NAMES);
    for tool in tools {
        let said = format!("{} {}", tool["name"], tool["description"]).to_lowercase();
        for word in ["delete", "prune", "clear", "replace", "approve"] {
            assert!(!said.contains(word), "{word} in {said}");
        }
        assert!(
            tool["description"].as_str().is_some_and(|d| !d.is_empty()),
            "{tool}"
        );
    }

    let before = world.tool_runs();
    let response = world.call(
        Some(&token()),
        "tools/call",
        json!({"name": "approve_roadmap", "arguments": {"project": "proj-alpha"}}),
    );
    let doc: Value = serde_json::from_str(body_of(&response)).unwrap();
    assert_eq!(doc["error"]["message"], "unknown tool", "{doc}");
    assert_eq!(world.tool_runs(), before);
}

#[test]
fn an_ask_through_the_connector_shows_on_the_hub_as_a_pending_question() {
    let world = World::new("mcp-ask");
    let question = "Which hour does the nag go out?";
    // The home page is read first, so a stale cached read would hide the ask.
    assert!(!body_of(&world.hub.get("/")).contains(question));

    let asked = world.tool(
        "ask",
        json!({"project": "proj-alpha", "question": question, "options": "seven,eight", "for": "human"}),
    );
    assert_eq!(asked["isError"], json!(false), "{asked}");

    let page = world.hub.get("/");
    assert_eq!(status_of(&page), 200, "{page}");
    assert!(body_of(&page).contains(question), "{page}");
    let pending = world.printed(&["--project=proj-alpha", "questions", "--pending"]);
    assert!(pending.contains(question), "{pending}");
}

#[test]
fn a_value_that_looks_like_a_flag_is_written_as_text() {
    let world = World::new("mcp-dashes");
    let wrote = |name: &str, mut arguments: Value| {
        arguments["project"] = json!("proj-alpha");
        let result = world.tool(name, arguments);
        assert_eq!(result["isError"], json!(false), "{name}: {result}");
        text_of(&result).to_string()
    };

    wrote("add_log", json!({"text": "--clear"}));
    let logged = world.printed(&["--project=proj-alpha", "log", "--limit=5"]);
    assert!(logged.contains("  --clear\n"), "{logged}");

    wrote("set_handoff", json!({"text": "--stdin"}));
    let handoff = world.printed(&["--project=proj-alpha", "handoff"]);
    assert!(handoff.contains("--stdin"), "{handoff}");

    wrote("decide", json!({"text": "--force", "by": "agent"}));
    let decided = world.printed(&["--project=proj-alpha", "log", "--kind=ruling"]);
    assert!(decided.contains("--force"), "{decided}");

    let asked = wrote("ask", json!({"question": "--why", "recommend": "--yes"}));
    let id = asked.trim().trim_start_matches('#').to_string();
    wrote(
        "answer",
        json!({"id": id, "text": "--because", "by": "--me"}),
    );
    let shown = world.printed(&["--project=proj-alpha", "show", "--", &id]);
    assert!(shown.contains("--why"), "{shown}");
    let pending = world.printed(&["--project=proj-alpha", "questions", "--pending"]);
    assert!(!pending.contains("--why"), "answered: {pending}");

    let filed = wrote(
        "add_finding",
        json!({"milestone": "m1-auth", "step": "--s", "text": "--broken"}),
    );
    let finding = filed
        .split_whitespace()
        .next()
        .unwrap()
        .trim_start_matches('#')
        .to_string();
    wrote("close_finding", json!({"id": finding, "by": "--abc123"}));
    let findings = world.printed(&["--project=proj-alpha", "finding", "list"]);
    assert!(findings.contains("--broken"), "{findings}");
    assert!(!findings.contains(" open "), "closed: {findings}");

    wrote(
        "add_evidence",
        json!({"task": "t1", "note": "--note", "content": "--content\n"}),
    );
    let evidence = world.printed(&["--project=proj-alpha", "evidence", "list"]);
    assert!(
        evidence.contains("t1") && evidence.contains("--note"),
        "{evidence}"
    );

    // Each text went in after `--`, so mem never read one as a flag.
    let runs = invocations(&world.mem_log);
    let add_log = runs
        .iter()
        .find(|argv| argv.iter().any(|a| a == "--clear"))
        .unwrap();
    assert_eq!(add_log, &["--project=proj-alpha", "log", "--", "--clear"]);
}

#[test]
fn a_bad_audience_or_name_is_refused_before_mem_runs() {
    let world = World::new("mcp-refuse");
    let before = world.tool_runs();
    for (name, arguments) in [
        (
            "ask",
            json!({"project": "proj-alpha", "question": "q", "for": "root"}),
        ),
        (
            "decide",
            json!({"project": "proj-alpha", "text": "t", "by": "root"}),
        ),
        ("show", json!({"project": "proj-alpha", "ids": ["--json"]})),
        ("wiki", json!({"project": "proj-alpha", "slug": "--stdin"})),
        (
            "tick_task",
            json!({"project": "proj-alpha", "task": "t1 --clear"}),
        ),
        (
            "tick_milestone",
            json!({"project": "proj-alpha", "milestone": "--clear"}),
        ),
        ("add_log", json!({"project": "--scope=all", "text": "t"})),
    ] {
        let result = world.tool(name, arguments);
        assert_eq!(result["isError"], json!(true), "{name}: {result}");
    }
    assert_eq!(world.tool_runs(), before);
}

#[test]
fn each_read_returns_what_mem_prints() {
    let world = World::new("mcp-reads");
    world.seed_plan_roadmap_wiki();
    let logged = world.printed(&["--project=proj-alpha", "log", "--", "alpha read this"]);
    let id = logged.trim_start_matches('#')[..8].to_string();

    let reads: [(&str, Value, Vec<&str>); 14] = [
        ("context", json!({}), vec!["context"]),
        ("context", json!({"full": true}), vec!["context", "--full"]),
        (
            "search",
            json!({"query": "alpha"}),
            vec!["search", "--", "alpha"],
        ),
        (
            "search",
            json!({"query": "alpha", "limit": 1}),
            vec!["search", "--limit=1", "--", "alpha"],
        ),
        ("show", json!({"ids": [id]}), vec!["show", "--", &id]),
        ("wiki", json!({}), vec!["wiki"]),
        (
            "wiki",
            json!({"slug": "auth#tokens"}),
            vec!["wiki", "--", "auth#tokens"],
        ),
        ("questions", json!({}), vec!["questions"]),
        (
            "questions",
            json!({"pending": true}),
            vec!["questions", "--pending"],
        ),
        ("plan", json!({}), vec!["plan"]),
        ("roadmap", json!({}), vec!["roadmap"]),
        (
            "roadmap",
            json!({"status": true}),
            vec!["roadmap", "--status"],
        ),
        ("log", json!({"limit": 2}), vec!["log", "--limit=2"]),
        ("handoff", json!({}), vec!["handoff"]),
    ];
    for (name, mut arguments, argv) in reads {
        arguments["project"] = json!("proj-alpha");
        let mut args = vec!["--project=proj-alpha"];
        args.extend(argv);
        // A context read racing the hub's background questions poll has come
        // back once without the seeded question, so a mismatch reads again.
        for attempt in 1..=3 {
            let printed = world.printed(&args);
            assert!(!printed.is_empty(), "{args:?} printed nothing");
            let result = world.tool(name, arguments.clone());
            assert_eq!(result["isError"], json!(false), "{name}: {result}");
            if attempt == 3 || text_of(&result) == printed {
                assert_eq!(text_of(&result), printed, "{name} {args:?}");
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    let tokens = world.printed(&["--project=proj-alpha", "wiki", "--", "auth#tokens"]);
    assert!(tokens.contains("One per app."), "{tokens}");
    let status = world.printed(&["--project=proj-alpha", "roadmap", "--status"]);
    assert!(status.contains("draft"), "{status}");
}

#[test]
fn a_tick_checks_off_a_task_or_a_milestone_in_place() {
    let world = World::new("mcp-tick");
    world.seed_plan_roadmap_wiki();

    let ticked = world.tool("tick_task", json!({"project": "proj-alpha", "task": "t1"}));
    assert_eq!(ticked["isError"], json!(false), "{ticked}");
    let plan = world.printed(&["--project=proj-alpha", "plan"]);
    assert_eq!(
        plan,
        "# plan: m1-auth\n\n- [x] t1 Sign in\n- [ ] t2 Sign out\n"
    );

    let ticked = world.tool(
        "tick_milestone",
        json!({"project": "proj-alpha", "milestone": "m1-auth"}),
    );
    assert_eq!(ticked["isError"], json!(false), "{ticked}");
    let roadmap = world.printed(&["--project=proj-alpha", "roadmap"]);
    assert_eq!(
        roadmap,
        "# roadmap: alpha\n\n- [x] m1-auth Sign-in\n- [ ] m2-billing Billing\n"
    );
    // A tick is not an approval.
    let status = world.printed(&["--project=proj-alpha", "roadmap", "--status"]);
    assert!(status.contains("draft"), "{status}");
}

#[test]
fn write_wiki_replaces_a_page_and_wiki_reads_it_back() {
    let world = World::new("mcp-wiki");
    world.seed_plan_roadmap_wiki();

    let page = "--clear\n\n# Auth\n\nSign in through Access.\n";
    let written = world.tool(
        "write_wiki",
        json!({"project": "proj-alpha", "slug": "auth", "note": "Access, not a PIN", "page": page}),
    );
    assert_eq!(written["isError"], json!(false), "{written}");
    let read = world.tool("wiki", json!({"project": "proj-alpha", "slug": "auth"}));
    assert_eq!(
        text_of(&read),
        world.printed(&["--project=proj-alpha", "wiki", "auth"])
    );
    assert!(text_of(&read).contains("Sign in through Access."), "{read}");
    assert!(text_of(&read).starts_with("--clear"), "{read}");

    let runs = world.tool_runs();
    let refused = world.tool(
        "write_wiki",
        json!({"project": "proj-alpha", "slug": "--stdin", "note": "x", "page": "y"}),
    );
    assert_eq!(refused["isError"], json!(true), "{refused}");
    assert_eq!(world.tool_runs(), runs, "a refused slug runs no mem");
}
