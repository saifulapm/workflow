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
    let home = dir.join(machine);
    std::fs::create_dir_all(home.join("config/qshell")).unwrap();
    std::fs::create_dir_all(home.join("run")).unwrap();
    std::fs::write(home.join("config/qshell/machine"), format!("{machine}\n")).unwrap();
    let bin = home.join("bin");
    let calls = home.join("mem.calls");
    fixture_bin(
        &bin,
        "mem",
        &format!("printf '%s\\n' \"$*\" >> '{}'", calls.display()),
    );
    let socket = home.join("local.sock");
    let runtime = home.join("run");
    let peers = peers
        .iter()
        .map(|p| format!("127.0.0.1:{}", p.port))
        .collect::<Vec<_>>()
        .join(",");
    Hub::spawn_env(
        &home,
        &[&bin],
        &["local", "--port", "0"],
        &[
            ("HUB_PEER_TOKEN", TOKEN),
            ("HUB_CLOUD_TOKEN", CLOUD),
            ("HUB_LOCAL_SOCKET", socket.to_str().unwrap()),
            ("HUB_HOST", "127.0.0.1"),
            ("HUB_PEERS", &peers),
            ("XDG_RUNTIME_DIR", runtime.to_str().unwrap()),
        ],
    )
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

fn device(hub: &Hub, verb: &str) -> Value {
    let body = format!(r#"{{"verb":"{verb}","args":[],"project":"alpha"}}"#);
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
        answer["stderr"], "device: no device online (device: nuc)\n",
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
