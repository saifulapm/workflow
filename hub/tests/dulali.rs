//! Dulali's route: the doorbell posts a person's question to Hermes, signed;
//! Dulali answers or passes on its own socket with its own token; the answer
//! waits out an override window before it reaches mem; Saiful's answer inside
//! the window wins; a silent Dulali rings the phone.
//!
//! Over the real mem and a fixture `curl` that records every argv, so the
//! webhook post and the ntfy ring are both on record. Short windows come in
//! through the `HUB_DULALI_*_MS` seams.

mod common;

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use common::{
    DEVICE, Hub, TempDir, body_of, fixture_bin, invocations, mem_in, real_mem, recording_mem,
    seed_project, status_of, wait_for,
};
use serde_json::{Value, json};

const SECRET: &str = "test-webhook-secret";
const WEBHOOK: &str = "http://127.0.0.1:9/webhooks/question";
const NTFY: &str = "http://127.0.0.1:9";

struct World {
    _dir: TempDir,
    home: PathBuf,
    bin: PathBuf,
    curl_log: PathBuf,
    mem_log: PathBuf,
    config: PathBuf,
    socket: PathBuf,
    mem: PathBuf,
}

impl World {
    fn new(tag: &str) -> World {
        let dir = TempDir::new(tag);
        let home = dir.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let mem = real_mem().expect("build mem first");
        let (bin, mem_log) = recording_mem(dir.path(), &home);
        let curl_log = dir.join("curl.log");
        fixture_bin(
            &bin,
            "curl",
            &format!(
                "printf '\\1' >> '{log}'\n\
                 for a in \"$@\"; do printf '%s\\0' \"$a\" >> '{log}'; done\n\
                 exit 0",
                log = curl_log.display(),
            ),
        );
        let socket = dir.join("run/hub.sock");
        std::fs::create_dir_all(socket.parent().unwrap()).unwrap();
        let config = dir.join("config.toml");
        std::fs::write(
            &config,
            format!(
                "topic = \"workflow-TESTTESTTESTTESTTESTTESTTE\"\nntfy_base = \"{NTFY}\"\n\
                 dulali_webhook = \"{WEBHOOK}\"\ndulali_socket = \"{}\"\n",
                socket.display()
            ),
        )
        .unwrap();
        seed_project(&mem, &home, "proj-alpha", "alpha did a thing");
        World {
            _dir: dir,
            home,
            bin,
            curl_log,
            mem_log,
            config,
            socket,
            mem,
        }
    }

    /// A hub on the Dulali route, with the secret and short clocks.
    fn hub(&self, env: &[(&str, &str)]) -> Hub {
        self.route("{\"route\":\"dulali\",\"machine\":null}");
        let mut env = env.to_vec();
        env.push(("HUB_POLL_MS", "120"));
        env.push(("HUB_DULALI_SECRET", SECRET));
        let hub = Hub::spawn_env(
            &self.home,
            &[&self.bin],
            &["--config", self.config.to_str().unwrap(), "--port", "0"],
            &env,
        );
        wait_for("the dulali socket", Duration::from_secs(5), || {
            self.socket.exists()
        });
        hub
    }

    fn route(&self, json: &str) {
        let path = self.home.join("state/hub/route.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, json).unwrap();
    }

    fn token(&self) -> String {
        let path = self.home.join("state/hub/dulali-token");
        wait_for("dulali's token", Duration::from_secs(5), || path.exists());
        std::fs::read_to_string(path).unwrap().trim().to_string()
    }

    /// Asks in proj-alpha and answers with the full id.
    fn ask(&self, args: &[&str]) -> String {
        let mut argv = vec!["ask"];
        argv.extend_from_slice(args);
        argv.push("--json");
        let out = mem_in(&self.mem, &self.home, &self.home.join("proj-alpha"), &argv);
        assert!(out.status.success(), "{out:?}");
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        v["id"].as_str().unwrap().to_string()
    }

    fn calls(&self) -> Vec<Vec<String>> {
        invocations(&self.curl_log)
    }

    fn webhook_posts(&self) -> Vec<Vec<String>> {
        self.calls()
            .into_iter()
            .filter(|argv| argv.iter().any(|a| a == WEBHOOK))
            .collect()
    }

    /// The bodies of every ntfy ring.
    fn rings(&self) -> Vec<String> {
        self.calls()
            .into_iter()
            .filter(|argv| argv.iter().any(|a| a.starts_with(NTFY) && a != WEBHOOK))
            .filter_map(|argv| {
                let at = argv.iter().position(|a| a == "-d")?;
                argv.get(at + 1).cloned()
            })
            .collect()
    }

    fn polls(&self) -> usize {
        invocations(&self.mem_log)
            .iter()
            .filter(|argv| argv.first().is_some_and(|a| a == "projects"))
            .count()
    }

    fn settle(&self) {
        let before = self.polls();
        wait_for("the doorbell to go round", Duration::from_secs(15), || {
            self.polls() >= before + 2
        });
    }

    fn sent_to_dulali(&self, id: &str) {
        wait_for("the webhook post", Duration::from_secs(10), || {
            self.webhook_posts()
                .iter()
                .any(|argv| argv.iter().any(|a| a == &format!("X-Request-ID: {id}")))
        });
    }

    /// One POST on Dulali's socket.
    fn dulali(&self, token: &str, body: &Value) -> (u16, Value) {
        let body = body.to_string();
        let mut stream = UnixStream::connect(&self.socket).unwrap();
        write!(
            stream,
            "POST /api/dulali HTTP/1.1\r\nHost: hub\r\nConnection: close\r\n\
             Content-Type: application/json\r\nContent-Length: {}\r\n\
             Authorization: Bearer {token}\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        let json = serde_json::from_str(body_of(&response)).unwrap_or(Value::Null);
        (status_of(&response), json)
    }

    /// The question's row, answered or not.
    fn question(&self, id: &str) -> Value {
        let out = mem_in(
            &self.mem,
            &self.home,
            &self.home.join("proj-alpha"),
            &["questions", "--json"],
        );
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        v["questions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|q| q["id"] == id)
            .cloned()
            .unwrap_or_else(|| panic!("{id} not listed: {v}"))
    }
}

fn header<'a>(argv: &'a [String], name: &str) -> Option<&'a str> {
    argv.iter()
        .find_map(|a| a.strip_prefix(&format!("{name}: ")))
}

#[test]
fn a_persons_question_goes_to_hermes_signed_and_rings_nothing() {
    let world = World::new("dulali-send");
    let _hub = world.hub(&[]);
    world.settle();
    let id = world.ask(&[
        "nag at eight?",
        "--options",
        "eight,nine",
        "--recommend",
        "eight",
    ]);
    world.sent_to_dulali(&id);

    let posts = world.webhook_posts();
    assert_eq!(posts.len(), 1, "{posts:?}");
    let argv = &posts[0];
    let body = argv
        .iter()
        .position(|a| a == "--data-binary")
        .map(|at| argv[at + 1].clone())
        .unwrap();
    let sent: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(sent["id"], id.as_str());
    assert_eq!(sent["project"], "proj-alpha");
    assert!(!body.contains("nag at eight"), "the text went out: {body}");
    let ts = header(argv, "X-Webhook-Timestamp").unwrap();
    assert_eq!(
        header(argv, "X-Webhook-Signature-V2").unwrap(),
        hub::dulali::sign(SECRET, ts, &body)
    );
    world.settle();
    assert!(world.rings().is_empty(), "{:?}", world.rings());
}

#[test]
fn saifuls_question_and_an_orchestrators_never_reach_dulali() {
    let world = World::new("dulali-audience");
    let _hub = world.hub(&[]);
    world.settle();
    world.ask(&["push to prod?", "--for", "saiful"]);
    world.ask(&["which helper?", "--for", "orchestrator"]);
    wait_for("the saiful ring", Duration::from_secs(10), || {
        !world.rings().is_empty()
    });
    world.settle();
    assert!(
        world.webhook_posts().is_empty(),
        "{:?}",
        world.webhook_posts()
    );
    assert_eq!(world.rings().len(), 1, "{:?}", world.rings());
}

#[test]
fn a_question_pending_before_the_hub_knew_dulali_stays_saifuls() {
    let world = World::new("dulali-backlog");
    // Asked and rung under the old hub: its id is in the seen file.
    let old = world.ask(&["old one?", "--options", "a,b", "--recommend", "a"]);
    let seen = world.home.join("state/hub/seen");
    std::fs::create_dir_all(seen.parent().unwrap()).unwrap();
    std::fs::write(&seen, format!("{old}\nfinished roadmaps seeded\n")).unwrap();
    let _hub = world.hub(&[]);
    world.settle();
    assert!(
        world.webhook_posts().is_empty(),
        "{:?}",
        world.webhook_posts()
    );
    let (status, _) = world.dulali(
        &world.token(),
        &json!({"id": old, "answer": "a", "checked": "nothing"}),
    );
    assert_eq!(status, 409);
}

#[test]
fn dulalis_answer_waits_the_window_with_an_override_ring() {
    let world = World::new("dulali-hold");
    let _hub = world.hub(&[("HUB_DULALI_WINDOW_MS", "2500")]);
    world.settle();
    let id = world.ask(&[
        "nag at eight?",
        "--options",
        "eight,nine",
        "--recommend",
        "eight",
    ]);
    world.sent_to_dulali(&id);

    let (status, reply) = world.dulali(
        &world.token(),
        &json!({"id": id, "answer": "eight", "checked": "recommend; no ruling on the nag"}),
    );
    assert_eq!(status, 200, "{reply}");
    let rings = world.rings();
    assert_eq!(rings.len(), 1, "{rings:?}");
    assert!(rings[0].contains("override within 2 min"), "{}", rings[0]);
    assert!(rings[0].contains("eight"), "{}", rings[0]);
    assert!(rings[0].contains("http"), "{}", rings[0]);
    // Inside the window nothing reaches mem.
    assert_eq!(world.question(&id)["answered"], false);

    wait_for("the window to close", Duration::from_secs(10), || {
        world.question(&id)["answered"] == true
    });
    let row = world.question(&id);
    assert_eq!(row["answer_by"], "dulali", "{row}");
    let answer = row["answer"].as_str().unwrap();
    assert!(answer.starts_with("eight"), "{answer}");
    assert!(answer.contains("no ruling on the nag"), "{answer}");
}

#[test]
fn saifuls_answer_inside_the_window_replaces_dulalis() {
    let world = World::new("dulali-override");
    let hub = world.hub(&[("HUB_DULALI_WINDOW_MS", "2000")]);
    world.settle();
    let id = world.ask(&[
        "nag at eight?",
        "--options",
        "eight,nine",
        "--recommend",
        "eight",
    ]);
    world.sent_to_dulali(&id);
    let (status, _) = world.dulali(
        &world.token(),
        &json!({"id": id, "answer": "eight", "checked": "recommend"}),
    );
    assert_eq!(status, 200);

    let response = hub.post_form("/answer", &format!("id={id}&text=nine"));
    assert_eq!(status_of(&response), 303, "{response}");
    std::thread::sleep(Duration::from_millis(2500));
    world.settle();
    let row = world.question(&id);
    assert_eq!(row["answer"], "nine", "{row}");
    assert_eq!(row["answer_by"], "saiful", "{row}");
    let answers = invocations(&world.mem_log)
        .into_iter()
        .filter(|argv| argv.first().is_some_and(|a| a == "answer"))
        .count();
    assert_eq!(answers, 1, "Dulali's answer reached mem too");
}

#[test]
fn saiful_and_dulali_in_the_same_second_leave_saifuls_answer() {
    for round in 0..3 {
        let world = World::new(&format!("dulali-race-{round}"));
        let hub = world.hub(&[("HUB_DULALI_WINDOW_MS", "1000")]);
        world.settle();
        let id = world.ask(&["nag?", "--options", "eight,nine", "--recommend", "eight"]);
        world.sent_to_dulali(&id);
        let token = world.token();
        let dulali = {
            let id = id.clone();
            let socket = world.socket.clone();
            std::thread::spawn(move || {
                let body = json!({"id": id, "answer": "eight", "checked": "recommend"}).to_string();
                let mut stream = UnixStream::connect(socket).unwrap();
                write!(
                    stream,
                    "POST /api/dulali HTTP/1.1\r\nHost: hub\r\nConnection: close\r\n\
                     Content-Length: {}\r\nAuthorization: Bearer {token}\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
                let mut out = String::new();
                stream.read_to_string(&mut out).unwrap();
            })
        };
        hub.post_form("/answer", &format!("id={id}&text=nine"));
        dulali.join().unwrap();
        std::thread::sleep(Duration::from_millis(1500));
        world.settle();
        let row = world.question(&id);
        assert_eq!(row["answer"], "nine", "round {round}: {row}");
    }
}

#[test]
fn a_pass_rings_the_phone_once() {
    let world = World::new("dulali-pass");
    let _hub = world.hub(&[]);
    world.settle();
    let id = world.ask(&["what colour?"]);
    world.sent_to_dulali(&id);
    let (status, _) = world.dulali(
        &world.token(),
        &json!({"id": id, "pass": true, "checked": "no recommended option"}),
    );
    assert_eq!(status, 200);
    world.settle();
    let rings = world.rings();
    assert_eq!(rings.len(), 1, "{rings:?}");
    assert!(rings[0].contains("Dulali passed"), "{}", rings[0]);
    assert_eq!(world.question(&id)["answered"], false);
}

#[test]
fn a_silent_dulali_rings_the_phone_after_the_silence() {
    let world = World::new("dulali-silent");
    let _hub = world.hub(&[("HUB_DULALI_SILENT_MS", "2000")]);
    world.settle();
    let id = world.ask(&["nag?", "--options", "a,b", "--recommend", "a"]);
    world.sent_to_dulali(&id);
    assert!(world.rings().is_empty());
    wait_for("the silence ring", Duration::from_secs(10), || {
        !world.rings().is_empty()
    });
    let asked = hub::dulali::ulid_ms(&id).unwrap();
    let rang = jiff::Timestamp::now().as_millisecond();
    assert!(rang - asked >= 2000, "rang after {} ms", rang - asked);
    world.settle();
    let rings = world.rings();
    assert_eq!(rings.len(), 1, "{rings:?}");
    assert!(rings[0].contains("Dulali did not answer"), "{}", rings[0]);
}

#[test]
fn dulalis_token_opens_nothing_but_its_socket() {
    let world = World::new("dulali-token");
    let hub = world.hub(&[]);
    let token = world.token();
    // The agent routes take a bearer token; the pages a cookie.
    for (path, body) in [
        ("/api/route", "{\"route\":\"phone\"}"),
        (
            "/api/agent",
            "{\"machine\":\"x\",\"agents\":[],\"checkouts\":[]}",
        ),
    ] {
        let response = hub.raw(&format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\
             Content-Type: application/json\r\nContent-Length: {}\r\n\
             Authorization: Bearer {token}\r\n\r\n{body}",
            hub.port,
            body.len()
        ));
        assert_eq!(status_of(&response), 401, "{path}: {response}");
    }
    let cookie = format!("hub_device={token}");
    // Start, Pause, Unpause and roadmap approval are home's control verbs;
    // Resume follows an answer, and the route is home's header.
    for (path, body) in [
        ("/route", "route=phone"),
        ("/p/proj-alpha/control", "do=go"),
        ("/p/proj-alpha/control", "do=pause"),
        ("/p/proj-alpha/control", "do=unpause"),
        ("/p/proj-alpha/control", "do=approve"),
        ("/answer", "id=x&text=y"),
    ] {
        let response = hub.post_form_with(
            path,
            body,
            &[("Origin", &hub.origin()), ("Cookie", &cookie)],
        );
        assert_eq!(status_of(&response), 403, "{path} {body}: {response}");
        assert!(response.contains("not paired"), "{path} {body}: {response}");
    }
    // Nor is Dulali's route on the TCP port, even with its token.
    let response = hub.raw(&format!(
        "POST /api/dulali HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\
         Content-Length: 2\r\nAuthorization: Bearer {token}\r\n\r\n{{}}",
        hub.port
    ));
    assert_eq!(status_of(&response), 404, "{response}");
    // And the socket refuses a device's token.
    let (status, _) = world.dulali(DEVICE, &json!({"id": "x", "pass": true, "checked": "-"}));
    assert_eq!(status, 401);
}
