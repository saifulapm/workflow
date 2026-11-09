//! `POST /device` goes to the machine the route names, whichever local hub
//! takes the call, and says so when no device is online.

mod common;

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use common::{Hub, TempDir, body_of, fixture_bin, status_of};
use serde_json::Value;

const TOKEN: &str = "peer-token-for-tests-0123456789abcdef";

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

#[test]
fn on_the_phone_route_no_device_is_online() {
    let dir = TempDir::new("device-phone");
    let mini = spawn(&dir, "mini", &[]);

    let answer = device(&mini, "whoami");
    assert_eq!(answer["exit"], 3, "{answer}");
    assert_eq!(
        answer["stderr"], "device: no device online (route: phone)\n",
        "{answer}"
    );
    assert_eq!(answer["stdout"], "", "{answer}");
    assert_eq!(answer["machine"], Value::Null, "{answer}");
    assert!(
        mem_calls(&dir, "mini").contains(
            "--project=alpha log -- device none whoami exit 3 refused: no device online (route: phone)"
        ),
        "{}",
        mem_calls(&dir, "mini")
    );
}

#[test]
fn a_call_goes_to_the_route_device_and_follows_the_route_when_it_moves() {
    let dir = TempDir::new("device-follow");
    let nuc = spawn(&dir, "nuc", &[]);
    let mini = spawn(&dir, "mini", &[&nuc]);

    // Chosen on the mini, naming the nuc: the mini tells its peer at once.
    let nuc_host = format!("127.0.0.1:{}", nuc.port);
    route_on(
        &socket_of(&dir, "mini"),
        &format!(r#"{{"route":"device","machine":"{nuc_host}"}}"#),
    );
    assert_eq!(route_machine(&socket_of(&dir, "mini")), nuc_host.as_str());

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
        !mem_calls(&dir, "mini").contains("log --"),
        "{}",
        mem_calls(&dir, "mini")
    );

    route_on(&socket_of(&dir, "mini"), r#"{"route":"device"}"#);
    let mini_host = route_machine(&socket_of(&dir, "mini"));
    common::wait_for("the nuc hears the move", Duration::from_secs(2), || {
        route_machine(&socket_of(&dir, "nuc")) == mini_host
    });
    let answer = device(&mini, "whoami");
    assert_eq!(answer["stdout"], "mini\n", "{answer}");
    assert_eq!(answer["machine"], "mini", "{answer}");
    assert!(
        mem_calls(&dir, "mini").contains("log -- device mini whoami exit 0"),
        "{}",
        mem_calls(&dir, "mini")
    );
}

#[test]
fn a_route_device_that_is_not_on_the_tailnet_is_not_online() {
    let dir = TempDir::new("device-offline");
    let mini = spawn(&dir, "mini", &[]);
    route_on(
        &socket_of(&dir, "mini"),
        r#"{"route":"device","machine":"127.0.0.1:9"}"#,
    );

    let started = std::time::Instant::now();
    let answer = device(&mini, "whoami");
    assert_eq!(answer["exit"], 3, "{answer}");
    assert_eq!(
        answer["stderr"], "device: no device online (route: 127.0.0.1:9)\n",
        "{answer}"
    );
    assert!(started.elapsed() < Duration::from_secs(5));
}
