//! `POST /device` goes to the device chosen apart from the route, the mini
//! until one is chosen, whichever local hub takes the call, and names the
//! chosen device when it is not online.

mod common;

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use common::{Hub, TempDir, body_of, fixture_bin, status_of};
use serde_json::Value;

const TOKEN: &str = "peer-token-for-tests-0123456789abcdef";
const CLOUD: &str = "cloud-token-for-tests-0123456789abcdef";

/// A local hub named `machine`, reachable by peers at 127.0.0.1, with
/// `peers` as its tailnet and a mem that records each call.
fn spawn(dir: &TempDir, machine: &str, peers: &[&Hub]) -> Hub {
    let peers = peers
        .iter()
        .map(|p| format!("127.0.0.1:{}", p.port))
        .collect::<Vec<_>>()
        .join(",");
    spawn_with_peers(dir, machine, &peers)
}

/// The same with `peers` as its `HUB_PEERS` list.
fn spawn_with_peers(dir: &TempDir, machine: &str, peers: &str) -> Hub {
    let home = dir.join(machine);
    std::fs::create_dir_all(home.join("config/qshell")).unwrap();
    std::fs::create_dir_all(home.join("run")).unwrap();
    std::fs::write(home.join("config/qshell/machine"), format!("{machine}\n")).unwrap();
    let bin = home.join("bin");
    let calls = home.join("mem.calls");
    // Answers `projects --json`, a project's `questions --json` and `show`
    // from files the tests write; see `projects`, `questions` and `answered`.
    fixture_bin(
        &bin,
        "mem",
        &format!(
            "printf '%s\\n' \"$*\" >> '{calls}'\n\
             f=\n\
             case \"$*\" in\n\
             'projects --json') f='{home}/projects.json' ;;\n\
             '--project '*' questions --json') f=\"{home}/questions-$2.json\" ;;\n\
             'show '*' --json') f=\"{home}/show-$2.json\" ;;\n\
             esac\n\
             if [ -n \"$f\" ] && [ -f \"$f\" ]; then cat \"$f\"; fi",
            calls = calls.display(),
            home = home.display(),
        ),
    );
    let socket = home.join("local.sock");
    let runtime = home.join("run");
    Hub::spawn_env(
        &home,
        &[&bin],
        &["local", "--port", "0"],
        &[
            ("HUB_PEER_TOKEN", TOKEN),
            ("HUB_CLOUD_TOKEN", CLOUD),
            ("HUB_LOCAL_SOCKET", socket.to_str().unwrap()),
            ("HUB_HOST", "127.0.0.1"),
            ("HUB_PEERS", peers),
            ("XDG_RUNTIME_DIR", runtime.to_str().unwrap()),
            // A tmux call reaches this hub's own server, never Saiful's.
            (
                "HUB_DEVICE_TMUX_SOCKET",
                tmux_socket(&home).to_str().unwrap(),
            ),
        ],
    )
}

fn tmux_socket(home: &Path) -> PathBuf {
    home.join("tmux.sock")
}

/// Kills the tmux server of the hub in `home` when dropped.
struct TmuxServer(PathBuf);

impl Drop for TmuxServer {
    fn drop(&mut self) {
        let _ = std::process::Command::new("tmux")
            .arg("-S")
            .arg(tmux_socket(&self.0))
            .arg("kill-server")
            .output();
    }
}

/// git in `dir`, with none of the user's own config.
fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn socket_of(dir: &TempDir, machine: &str) -> PathBuf {
    dir.join(machine).join("local.sock")
}

fn mem_calls(dir: &TempDir, machine: &str) -> String {
    std::fs::read_to_string(dir.join(machine).join("mem.calls")).unwrap_or_default()
}

fn route_on(socket: &Path, body: &str) {
    let mut stream = UnixStream::connect(socket).unwrap();
    write!(
        stream,
        "POST /route HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert_eq!(status_of(&response), 200, "{response}");
}

fn route_machine(socket: &Path) -> Value {
    let mut stream = UnixStream::connect(socket).unwrap();
    stream
        .write_all(b"GET /route HTTP/1.1\r\nHost: x\r\n\r\n")
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    serde_json::from_str::<Value>(body_of(&response)).unwrap()["machine"].clone()
}

/// What `mem projects --json` answers on `machine`.
fn projects(dir: &TempDir, machine: &str, json: &str) {
    std::fs::write(dir.join(machine).join("projects.json"), json).unwrap();
}

/// What `mem --project <project> questions --json` answers on `machine`.
fn questions(dir: &TempDir, machine: &str, project: &str, json: &str) {
    let file = dir.join(machine).join(format!("questions-{project}.json"));
    std::fs::write(file, json).unwrap();
}

/// What `mem show <answer> --json` answers on `machine`: an answer item
/// given on `by`.
fn answered(dir: &TempDir, machine: &str, answer: &str, by: &str) {
    let file = dir.join(machine).join(format!("show-{answer}.json"));
    let json = format!(r#"{{"items":[{{"machine":"{by}","by":"slack","kind":"answer"}}]}}"#);
    std::fs::write(file, json).unwrap();
}

fn device(hub: &Hub, verb: &str) -> Value {
    device_for(hub, verb, "alpha")
}

fn device_for(hub: &Hub, verb: &str, project: &str) -> Value {
    device_body(
        hub,
        &format!(r#"{{"verb":"{verb}","args":[],"project":"{project}"}}"#),
    )
}

/// A `whoami` of `project`'s run that asked `question` which device to use.
fn device_asked(hub: &Hub, project: &str, question: &str) -> Value {
    device_body(
        hub,
        &format!(r#"{{"verb":"whoami","args":[],"project":"{project}","question":"{question}"}}"#),
    )
}

fn device_body(hub: &Hub, body: &str) -> Value {
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", hub.port)).unwrap();
    write!(
        stream,
        "POST /device HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {TOKEN}\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert_eq!(status_of(&response), 200, "{response}");
    serde_json::from_str(body_of(&response)).unwrap()
}

fn on_socket(socket: &Path, request: &str) -> String {
    let mut stream = UnixStream::connect(socket).unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

/// Chooses the device on this machine, as `workflow device use` does.
fn choose(socket: &Path, device: &str) -> Value {
    let body = format!(r#"{{"device":"{device}"}}"#);
    let response = on_socket(
        socket,
        &format!(
            "POST /device/choice HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ),
    );
    assert_eq!(status_of(&response), 200, "{response}");
    serde_json::from_str(body_of(&response)).unwrap()
}

fn chosen(socket: &Path) -> Value {
    let response = on_socket(socket, "GET /device/choice HTTP/1.1\r\nHost: x\r\n\r\n");
    assert_eq!(status_of(&response), 200, "{response}");
    serde_json::from_str::<Value>(body_of(&response)).unwrap()["device"].clone()
}

/// A request on `hub`'s peer port with `bearer`, if any: its whole response.
fn on_peer_port(hub: &Hub, bearer: Option<&str>, request_line: &str, body: &str) -> String {
    let auth = bearer
        .map(|b| format!("Authorization: Bearer {b}\r\n"))
        .unwrap_or_default();
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", hub.port)).unwrap();
    write!(
        stream,
        "{request_line}\r\nHost: x\r\n{auth}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

#[test]
fn a_hub_that_never_had_a_device_chosen_serves_calls_on_the_mini() {
    let dir = TempDir::new("device-default");
    let mini = spawn(&dir, "mac-mini-m2", &[]);
    assert_eq!(chosen(&socket_of(&dir, "mac-mini-m2")), "mac-mini-m2");

    let answer = device(&mini, "whoami");
    assert_eq!(answer["stdout"], "mac-mini-m2\n", "{answer}");
    assert_eq!(answer["machine"], "mac-mini-m2", "{answer}");
    assert_eq!(answer["exit"], 0, "{answer}");
}

#[test]
fn on_the_phone_route_a_call_goes_to_the_chosen_device_and_follows_it() {
    let dir = TempDir::new("device-follow");
    let nuc = spawn(&dir, "nuc", &[]);
    let mini = spawn(&dir, "mac-mini-m2", &[&nuc]);
    let socket = socket_of(&dir, "mac-mini-m2");
    assert_eq!(route_machine(&socket), Value::Null, "the route is Phone");

    let set = choose(&socket, "nuc");
    assert_eq!(set["device"], "nuc", "{set}");
    assert!(set["at"].as_i64().unwrap() > 0, "{set}");
    assert_eq!(chosen(&socket), "nuc");

    let answer = device(&mini, "whoami");
    assert_eq!(answer["stdout"], "nuc\n", "{answer}");
    assert_eq!(answer["machine"], "nuc", "{answer}");
    assert_eq!(answer["exit"], 0, "{answer}");
    // The machine that ran it logs it, once.
    assert!(
        mem_calls(&dir, "nuc").contains("log -- device nuc whoami exit 0"),
        "{}",
        mem_calls(&dir, "nuc")
    );
    assert!(
        !mem_calls(&dir, "mac-mini-m2").contains("log --"),
        "{}",
        mem_calls(&dir, "mac-mini-m2")
    );

    choose(&socket, "mac-mini-m2");
    let answer = device(&mini, "whoami");
    assert_eq!(answer["stdout"], "mac-mini-m2\n", "{answer}");
    assert_eq!(
        route_machine(&socket),
        Value::Null,
        "the route stayed Phone"
    );
}

#[test]
fn a_route_on_another_machine_does_not_move_the_device() {
    let dir = TempDir::new("device-route-elsewhere");
    let mini = spawn(&dir, "mac-mini-m2", &[]);
    let macbook = spawn(&dir, "macbook-m2", &[&mini]);
    // Device on the macbook, as an old route file holds it.
    route_on(&socket_of(&dir, "macbook-m2"), r#"{"route":"device"}"#);
    assert_ne!(route_machine(&socket_of(&dir, "macbook-m2")), Value::Null);

    let answer = device(&macbook, "whoami");
    assert_eq!(answer["stdout"], "mac-mini-m2\n", "{answer}");
    assert_eq!(answer["exit"], 0, "{answer}");

    choose(&socket_of(&dir, "macbook-m2"), "mac-mini-m2");
    let answer = device(&macbook, "whoami");
    assert_eq!(answer["stdout"], "mac-mini-m2\n", "{answer}");
}

#[test]
fn a_chosen_device_that_is_offline_says_so_by_its_name() {
    let dir = TempDir::new("device-offline");
    let nuc = spawn(&dir, "nuc", &[]);
    let mini = spawn(&dir, "mac-mini-m2", &[&nuc]);
    choose(&socket_of(&dir, "mac-mini-m2"), "nuc");
    drop(nuc);

    let started = std::time::Instant::now();
    let answer = device(&mini, "whoami");
    assert_eq!(answer["exit"], 3, "{answer}");
    assert_eq!(
        answer["stderr"], "device: no device online (device: nuc); online: mac-mini-m2\n",
        "{answer}"
    );
    assert_eq!(answer["device"], "nuc", "{answer}");
    assert_eq!(
        answer["online"],
        serde_json::json!(["mac-mini-m2"]),
        "{answer}"
    );
    assert_eq!(answer["stdout"], "", "{answer}");
    assert_eq!(answer["machine"], Value::Null, "{answer}");
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(
        mem_calls(&dir, "mac-mini-m2").contains(
            "--project=alpha log -- device none whoami exit 3 refused: no device online (device: nuc)"
        ),
        "{}",
        mem_calls(&dir, "mac-mini-m2")
    );
}

const PALMSHOP_ON_NUC: &str = r#"{"projects":[
    {"name":"palmshop","device":"nuc","device_by":"mac-mini-m2"},
    {"name":"shortcart","device":null,"device_by":null}
]}"#;

#[test]
fn a_project_with_its_own_device_sends_its_calls_there() {
    let dir = TempDir::new("device-project");
    let nuc = spawn(&dir, "nuc", &[]);
    let mini = spawn(&dir, "mac-mini-m2", &[&nuc]);
    projects(&dir, "mac-mini-m2", PALMSHOP_ON_NUC);
    assert_eq!(chosen(&socket_of(&dir, "mac-mini-m2")), "mac-mini-m2");

    let answer = device_for(&mini, "whoami", "palmshop");
    assert_eq!(answer["machine"], "nuc", "{answer}");
    let answer = device_for(&mini, "whoami", "shortcart");
    assert_eq!(answer["machine"], "mac-mini-m2", "{answer}");
}

#[test]
fn a_tmux_send_for_a_project_on_nuc_makes_its_worktree_on_nuc_only() {
    let dir = TempDir::new("device-project-tmux");
    git(
        dir.path(),
        &["init", "-q", "--bare", "-b", "main", "origin.git"],
    );
    git(dir.path(), &["clone", "-q", "origin.git", "checkout"]);
    let checkout = dir.join("checkout");
    std::fs::write(checkout.join("README"), "palm\n").unwrap();
    git(&checkout, &["add", "."]);
    git(&checkout, &["commit", "-q", "-m", "first"]);
    git(
        &checkout,
        &["push", "-q", "origin", "HEAD:refs/heads/cloud/palmshop-m1"],
    );
    let nuc = spawn(&dir, "nuc", &[]);
    let _server = TmuxServer(dir.join("nuc"));
    let mini = spawn(&dir, "mac-mini-m2", &[&nuc]);
    let rows = serde_json::json!({"projects": [
        {"name": "palmshop", "device": "nuc", "device_by": "mac-mini-m2", "milestone": "m1",
         "remote": "github.com/saifulapm/palm", "checkouts": [checkout]},
    ]})
    .to_string();
    projects(&dir, "mac-mini-m2", &rows);
    projects(&dir, "nuc", &rows);

    let answer = device_body(
        &mini,
        r#"{"verb":"tmux","args":["send","true"],"project":"palmshop","session":"cse_01Palm"}"#,
    );
    assert_eq!(answer["machine"], "nuc", "{answer}");
    assert_eq!(answer["exit"], 0, "{answer}");
    assert!(dir.join("nuc/.cache/workflow-cloud/palm/README").is_file());
    assert!(!dir.join("mac-mini-m2/.cache/workflow-cloud/palm").exists());
}

#[test]
fn a_project_without_a_device_follows_the_global_choice() {
    let dir = TempDir::new("device-project-global");
    let nuc = spawn(&dir, "nuc", &[]);
    let mini = spawn(&dir, "mac-mini-m2", &[&nuc]);
    projects(&dir, "mac-mini-m2", PALMSHOP_ON_NUC);
    choose(&socket_of(&dir, "mac-mini-m2"), "nuc");

    let answer = device_for(&mini, "whoami", "shortcart");
    assert_eq!(answer["machine"], "nuc", "{answer}");
}

#[test]
fn a_project_device_set_by_a_cloud_thread_is_not_followed() {
    let dir = TempDir::new("device-project-cloud");
    let nuc = spawn(&dir, "nuc", &[]);
    let mini = spawn(&dir, "mac-mini-m2", &[&nuc]);
    projects(
        &dir,
        "mac-mini-m2",
        r#"{"projects":[{"name":"palmshop","device":"nuc","device_by":"cloud-abc"}]}"#,
    );

    let answer = device_for(&mini, "whoami", "palmshop");
    assert_eq!(answer["machine"], "mac-mini-m2", "{answer}");
}

#[test]
fn a_project_device_that_is_offline_says_so_by_its_name() {
    let dir = TempDir::new("device-project-offline");
    let nuc = spawn(&dir, "nuc", &[]);
    let mini = spawn(&dir, "mac-mini-m2", &[&nuc]);
    projects(&dir, "mac-mini-m2", PALMSHOP_ON_NUC);
    drop(nuc);

    let answer = device_for(&mini, "whoami", "palmshop");
    assert_eq!(answer["exit"], 3, "{answer}");
    assert_eq!(
        answer["stderr"], "device: no device online (device: nuc); online: mac-mini-m2\n",
        "{answer}"
    );
}

#[test]
fn an_offline_device_names_the_devices_that_are_online() {
    let dir = TempDir::new("device-online");
    // The global choice stays the mini, which is not running.
    let nuc = spawn(&dir, "nuc", &[]);
    let answer = device(&nuc, "whoami");
    assert_eq!(answer["exit"], 3, "{answer}");
    assert_eq!(
        answer["stderr"], "device: no device online (device: mac-mini-m2); online: nuc\n",
        "{answer}"
    );
    assert_eq!(answer["device"], "mac-mini-m2", "{answer}");
    assert_eq!(answer["online"], serde_json::json!(["nuc"]), "{answer}");

    drop(nuc);
    let macbook = spawn(&dir, "macbook-m2", &[]);
    let nuc = spawn(&dir, "nuc", &[&macbook]);
    let answer = device(&nuc, "whoami");
    assert_eq!(
        answer["stderr"],
        "device: no device online (device: mac-mini-m2); online: nuc, macbook-m2\n",
        "{answer}"
    );
    assert_eq!(
        answer["online"],
        serde_json::json!(["nuc", "macbook-m2"]),
        "{answer}"
    );
}

#[test]
fn a_project_device_that_is_not_a_device_is_not_followed() {
    let dir = TempDir::new("device-project-unknown");
    let mini = spawn(&dir, "mac-mini-m2", &[]);
    projects(
        &dir,
        "mac-mini-m2",
        r#"{"projects":[{"name":"palmshop","device":"cloud-vm","device_by":"nuc"}]}"#,
    );

    let answer = device_for(&mini, "whoami", "palmshop");
    assert_eq!(answer["machine"], "mac-mini-m2", "{answer}");
}

#[test]
fn only_this_machine_or_a_device_peer_chooses_the_device() {
    let dir = TempDir::new("device-who-chooses");
    let mini = spawn(&dir, "mac-mini-m2", &[]);
    let line = "POST /device/choice HTTP/1.1";
    let stamped = r#"{"device":"nuc","at":99999999999999}"#;

    let cloud = on_peer_port(&mini, Some(CLOUD), line, stamped);
    assert_eq!(status_of(&cloud), 403, "{cloud}");
    let nobody = on_peer_port(&mini, None, line, stamped);
    assert_eq!(status_of(&nobody), 401, "{nobody}");
    // A peer's choice carries the time it was made.
    let unstamped = on_peer_port(&mini, Some(TOKEN), line, r#"{"device":"nuc"}"#);
    assert_eq!(status_of(&unstamped), 400, "{unstamped}");
    assert_eq!(chosen(&socket_of(&dir, "mac-mini-m2")), "mac-mini-m2");

    for body in [r#"{"device":""}"#, r#"{"route":"phone"}"#, "nonsense"] {
        let response = on_socket(
            &socket_of(&dir, "mac-mini-m2"),
            &format!(
                "{line}\r\nHost: x\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            ),
        );
        assert_eq!(status_of(&response), 400, "{body}: {response}");
    }
    assert_eq!(chosen(&socket_of(&dir, "mac-mini-m2")), "mac-mini-m2");
}

#[test]
fn a_device_chosen_on_one_hub_reaches_every_peer_and_leaves_the_route() {
    let dir = TempDir::new("device-spread");
    let nuc = spawn(&dir, "nuc", &[]);
    let macbook = spawn(&dir, "macbook-m2", &[]);
    let _mini = spawn(&dir, "mac-mini-m2", &[&nuc, &macbook]);
    let route_before = route_machine(&socket_of(&dir, "nuc"));

    choose(&socket_of(&dir, "mac-mini-m2"), "nuc");
    for peer in ["nuc", "macbook-m2"] {
        common::wait_for(
            &format!("{peer} hears the choice"),
            Duration::from_secs(2),
            || chosen(&socket_of(&dir, peer)) == "nuc",
        );
    }
    assert_eq!(route_machine(&socket_of(&dir, "nuc")), route_before);
}

#[test]
fn an_older_choice_from_a_peer_loses_to_the_newer_one_here() {
    let dir = TempDir::new("device-older");
    let mini = spawn(&dir, "mac-mini-m2", &[]);
    choose(&socket_of(&dir, "mac-mini-m2"), "nuc");

    let stale = on_peer_port(
        &mini,
        Some(TOKEN),
        "POST /device/choice HTTP/1.1",
        r#"{"device":"macbook-m2","at":1}"#,
    );
    assert_eq!(status_of(&stale), 200, "{stale}");
    assert_eq!(chosen(&socket_of(&dir, "mac-mini-m2")), "nuc");

    let newer = on_peer_port(
        &mini,
        Some(TOKEN),
        "POST /device/choice HTTP/1.1",
        r#"{"device":"macbook-m2","at":99999999999999}"#,
    );
    assert_eq!(status_of(&newer), 200, "{newer}");
    assert_eq!(chosen(&socket_of(&dir, "mac-mini-m2")), "macbook-m2");
}

#[test]
fn a_hub_keeps_its_choice_across_a_restart_and_a_late_one_learns_the_newest() {
    let dir = TempDir::new("device-late");
    let nuc = spawn(&dir, "nuc", &[]);
    choose(&socket_of(&dir, "nuc"), "macbook-m2");
    drop(nuc);
    let nuc = spawn(&dir, "nuc", &[]);
    assert_eq!(chosen(&socket_of(&dir, "nuc")), "macbook-m2");

    // The mini was away when the choice was made; it catches up at start.
    let _mini = spawn(&dir, "mac-mini-m2", &[&nuc]);
    common::wait_for(
        "the late mini learns the choice",
        Duration::from_secs(2),
        || chosen(&socket_of(&dir, "mac-mini-m2")) == "macbook-m2",
    );
}

/// A peer whose hub predates device calls: it answers `/ping` with `name`
/// and 404 to everything else.
fn old_hub(name: &'static str) -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut head = [0u8; 4096];
            let n = stream.read(&mut head).unwrap_or(0);
            let ping = head[..n].starts_with(b"GET /ping ");
            let (status, body) = if ping {
                ("200 OK", name)
            } else {
                ("404 Not Found", "not found")
            };
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    port
}

#[test]
fn a_chosen_device_whose_hub_cannot_serve_calls_is_not_online() {
    let dir = TempDir::new("device-old-peer");
    let port = old_hub("nuc");
    let mini = spawn_with_peers(&dir, "mac-mini-m2", &format!("127.0.0.1:{port}"));
    choose(&socket_of(&dir, "mac-mini-m2"), "nuc");

    let answer = device(&mini, "whoami");
    assert_eq!(answer["exit"], 3, "{answer}");
    // Its ping answers to the name, but it served nothing: not online.
    assert_eq!(
        answer["stderr"], "device: no device online (device: nuc); online: mac-mini-m2\n",
        "{answer}"
    );
}

const QUESTION: &str = "ABCD1234";
/// Minted by the real mem on 2026-08-18.
const ASKED_ULID: &str = "01M0BF1F8BY8FXGZS428J1TSD1";
const ANSWER_ID: &str = "01M0BF2A00AAAAAAAAAAAAAAAA";
const BEFORE_ASKED: &str = "2026-08-01T00:00:00Z";
const AFTER_ASKED: &str = "2026-09-01T00:00:00Z";

/// On the mini, whose global choice stays the mini: `listed_in`'s questions
/// hold one of `project`'s, answered `answer` on machine `by`, and both
/// palmshop and shortcart last handed off at `handoff_at`.
fn asked_run(
    dir: &TempDir,
    listed_in: &str,
    project: &str,
    answer: &str,
    by: &str,
    handoff_at: &str,
) {
    questions(
        dir,
        "mac-mini-m2",
        listed_in,
        &serde_json::json!({"questions": [{
            "short_id": QUESTION, "id": ASKED_ULID, "project": project,
            "answered": true, "answer": answer, "answer_id": ANSWER_ID,
            "options": ["nuc", "I will turn it on"],
        }]})
        .to_string(),
    );
    answered(dir, "mac-mini-m2", ANSWER_ID, by);
    projects(
        dir,
        "mac-mini-m2",
        &serde_json::json!({"projects": [
            {"name": "palmshop", "device": null, "device_by": null, "handoff_at": handoff_at},
            {"name": "shortcart", "device": null, "device_by": null, "handoff_at": handoff_at},
        ]})
        .to_string(),
    );
}

#[test]
fn a_run_s_answered_device_takes_its_project_s_calls() {
    let dir = TempDir::new("device-run-answer");
    let nuc = spawn(&dir, "nuc", &[]);
    let mini = spawn(&dir, "mac-mini-m2", &[&nuc]);
    asked_run(&dir, "palmshop", "palmshop", "nuc", "nuc", BEFORE_ASKED);

    let answer = device_asked(&mini, "palmshop", QUESTION);
    assert_eq!(answer["machine"], "nuc", "{answer}");
    let answer = device_for(&mini, "whoami", "shortcart");
    assert_eq!(answer["machine"], "mac-mini-m2", "{answer}");
}

#[test]
fn a_run_s_answer_given_in_a_cloud_thread_is_not_followed() {
    let dir = TempDir::new("device-run-cloud-answer");
    let nuc = spawn(&dir, "nuc", &[]);
    let mini = spawn(&dir, "mac-mini-m2", &[&nuc]);
    asked_run(
        &dir,
        "palmshop",
        "palmshop",
        "nuc",
        "cloud-vm",
        BEFORE_ASKED,
    );

    let answer = device_asked(&mini, "palmshop", QUESTION);
    assert_eq!(answer["machine"], "mac-mini-m2", "{answer}");
}

#[test]
fn a_run_s_device_lapses_at_a_handoff_after_the_answer() {
    let dir = TempDir::new("device-run-handoff");
    let nuc = spawn(&dir, "nuc", &[]);
    let mini = spawn(&dir, "mac-mini-m2", &[&nuc]);
    asked_run(&dir, "palmshop", "palmshop", "nuc", "nuc", AFTER_ASKED);

    let answer = device_asked(&mini, "palmshop", QUESTION);
    assert_eq!(answer["machine"], "mac-mini-m2", "{answer}");
}

/// The question parked the run (its handoff came between the ask and the
/// answer), and Saiful answered after: the answer is followed.
#[test]
fn a_run_s_device_answered_after_the_park_it_caused_is_followed() {
    let dir = TempDir::new("device-run-late-answer");
    let nuc = spawn(&dir, "nuc", &[]);
    let mini = spawn(&dir, "mac-mini-m2", &[&nuc]);
    // Between the ask (22:14:30) and the answer (22:14:58).
    asked_run(
        &dir,
        "palmshop",
        "palmshop",
        "nuc",
        "nuc",
        "2026-08-18T22:14:45Z",
    );

    let answer = device_asked(&mini, "palmshop", QUESTION);
    assert_eq!(answer["machine"], "nuc", "{answer}");
}

#[test]
fn a_run_s_answer_that_is_not_a_device_is_not_followed() {
    let dir = TempDir::new("device-run-turn-on");
    let nuc = spawn(&dir, "nuc", &[]);
    let mini = spawn(&dir, "mac-mini-m2", &[&nuc]);
    asked_run(
        &dir,
        "palmshop",
        "palmshop",
        "I will turn it on",
        "nuc",
        BEFORE_ASKED,
    );

    let answer = device_asked(&mini, "palmshop", QUESTION);
    assert_eq!(answer["machine"], "mac-mini-m2", "{answer}");
}

#[test]
fn a_run_s_device_that_is_offline_says_so_by_its_name() {
    let dir = TempDir::new("device-run-offline");
    let nuc = spawn(&dir, "nuc", &[]);
    let mini = spawn(&dir, "mac-mini-m2", &[&nuc]);
    asked_run(
        &dir,
        "palmshop",
        "palmshop",
        "macbook-m2",
        "nuc",
        BEFORE_ASKED,
    );

    let answer = device_asked(&mini, "palmshop", QUESTION);
    assert_eq!(answer["exit"], 3, "{answer}");
    assert_eq!(
        answer["stderr"],
        "device: no device online (device: macbook-m2); online: mac-mini-m2, nuc\n",
        "{answer}"
    );
}

#[test]
fn a_run_s_question_of_another_project_is_not_followed() {
    let dir = TempDir::new("device-run-other-project");
    let nuc = spawn(&dir, "nuc", &[]);
    let mini = spawn(&dir, "mac-mini-m2", &[&nuc]);
    asked_run(&dir, "shortcart", "palmshop", "nuc", "nuc", BEFORE_ASKED);

    let answer = device_asked(&mini, "shortcart", QUESTION);
    assert_eq!(answer["machine"], "mac-mini-m2", "{answer}");
}
