//! `hub local`: the local hub's peer port and its socket for mem.

mod common;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Command, Stdio};

use common::{Hub, TempDir, body_of, hub_bin, status_of};

const TOKEN: &str = "peer-token-for-tests-0123456789abcdef";

fn home_named(dir: &TempDir, machine: &str) -> std::path::PathBuf {
    let home = dir.join("home");
    std::fs::create_dir_all(home.join("config/qshell")).unwrap();
    std::fs::write(home.join("config/qshell/machine"), format!("{machine}\n")).unwrap();
    home
}

fn spawn_local(dir: &TempDir, machine: &str) -> Hub {
    spawn_local_with(dir, machine, &[], &[])
}

fn spawn_local_with(dir: &TempDir, machine: &str, path: &[&Path], env: &[(&str, &str)]) -> Hub {
    let home = home_named(dir, machine);
    let socket = dir.join("local.sock");
    let mut all = vec![
        ("HUB_PEER_TOKEN", TOKEN),
        ("HUB_LOCAL_SOCKET", socket.to_str().unwrap()),
    ];
    all.extend_from_slice(env);
    Hub::spawn_env(&home, path, &["local", "--port", "0"], &all)
}

fn ping(home: &Path, target: &str, env: &[(&str, &str)]) -> std::process::Output {
    let mut command = Command::new(hub_bin());
    command
        .args(["ping", target])
        .env_clear()
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin")
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_STATE_HOME", home.join("state"))
        .stdin(Stdio::null());
    for (name, value) in env {
        command.env(name, value);
    }
    command.output().expect("run hub ping")
}

#[test]
fn a_peer_with_the_token_gets_the_machine_name() {
    let dir = TempDir::new("local-ping");
    let hub = spawn_local(&dir, "testbox");
    let caller = TempDir::new("local-ping-caller");
    let out = ping(
        caller.path(),
        &format!("127.0.0.1:{}", hub.port),
        &[("HUB_PEER_TOKEN", TOKEN)],
    );
    assert!(
        out.status.success(),
        "hub ping failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "testbox");
}

#[test]
fn a_request_without_the_right_token_is_refused() {
    let dir = TempDir::new("local-refuse");
    let hub = spawn_local(&dir, "testbox");
    let none = hub.raw("GET /ping HTTP/1.1\r\nHost: x\r\n\r\n");
    assert_eq!(status_of(&none), 401, "{none}");
    let wrong = hub.raw("GET /ping HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer nope\r\n\r\n");
    assert_eq!(status_of(&wrong), 401, "{wrong}");
    assert!(!body_of(&wrong).contains("testbox"));

    let caller = TempDir::new("local-refuse-caller");
    let out = ping(
        caller.path(),
        &format!("127.0.0.1:{}", hub.port),
        &[("HUB_PEER_TOKEN", "a-different-token")],
    );
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("401"));
}

#[test]
fn the_local_hub_will_not_start_without_a_peer_token() {
    let dir = TempDir::new("local-notoken");
    let home = home_named(&dir, "testbox");
    let out = Command::new(hub_bin())
        .args(["local", "--port", "0"])
        .env_clear()
        .env("HOME", &home)
        .env("PATH", "/usr/bin:/bin")
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("HUB_LOCAL_SOCKET", dir.join("local.sock"))
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("HUB_PEER_TOKEN"));
}

#[test]
fn mem_reaches_the_local_hub_on_its_socket_without_a_token() {
    let dir = TempDir::new("local-socket");
    let _hub = spawn_local(&dir, "testbox");
    let socket = dir.join("local.sock");
    let mode = std::fs::metadata(&socket).unwrap();
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(mode.permissions().mode() & 0o777, 0o600);
    let mut stream = UnixStream::connect(&socket).unwrap();
    stream
        .write_all(b"GET /ping HTTP/1.1\r\nHost: x\r\n\r\n")
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert_eq!(status_of(&response), 200, "{response}");
    assert_eq!(body_of(&response).trim(), "testbox");
}

/// A cloud VM runs Tailscale in userspace, where a tailnet address is reached
/// only through tailscaled's SOCKS5 listener.
#[test]
fn hub_ping_goes_through_a_socks5_proxy_when_one_is_named() {
    let dir = TempDir::new("local-socks");
    let hub = spawn_local(&dir, "testbox");
    let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy_port = proxy.local_addr().unwrap().port();
    let hub_port = hub.port;
    let asked = std::thread::spawn(move || {
        let (mut client, _) = proxy.accept().unwrap();
        let mut greeting = [0u8; 3];
        client.read_exact(&mut greeting).unwrap();
        assert_eq!(greeting, [5, 1, 0]);
        client.write_all(&[5, 0]).unwrap();
        let mut head = [0u8; 5];
        client.read_exact(&mut head).unwrap();
        assert_eq!(&head[..4], &[5, 1, 0, 3], "a CONNECT by name");
        let mut name = vec![0u8; head[4] as usize];
        client.read_exact(&mut name).unwrap();
        let mut port = [0u8; 2];
        client.read_exact(&mut port).unwrap();
        client.write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 0]).unwrap();
        let mut upstream = TcpStream::connect(("127.0.0.1", hub_port)).unwrap();
        let mut up = upstream.try_clone().unwrap();
        let mut down = client.try_clone().unwrap();
        let pump = std::thread::spawn(move || std::io::copy(&mut client, &mut up));
        std::io::copy(&mut upstream, &mut down).unwrap();
        down.shutdown(std::net::Shutdown::Both).unwrap();
        let _ = pump.join();
        (String::from_utf8(name).unwrap(), u16::from_be_bytes(port))
    });

    let caller = TempDir::new("local-socks-caller");
    let out = ping(
        caller.path(),
        &format!("mini.tailnet:{hub_port}"),
        &[
            ("HUB_PEER_TOKEN", TOKEN),
            ("HUB_SOCKS5", &format!("127.0.0.1:{proxy_port}")),
        ],
    );
    assert!(
        out.status.success(),
        "hub ping failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "testbox");
    let (name, port) = asked.join().unwrap();
    assert_eq!(name, "mini.tailnet");
    assert_eq!(port, hub_port);
}

fn on_socket(socket: &Path, request: &str) -> String {
    let mut stream = UnixStream::connect(socket).unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

fn post_answer(hub: &Hub, token: &str, body: &str) -> String {
    hub.raw(&format!(
        "POST /answer HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    ))
}

#[test]
fn a_waiting_mem_gets_the_answer_a_peer_hands_in() {
    let dir = TempDir::new("local-answer");
    let hub = spawn_local(&dir, "testbox");
    let socket = dir.join("local.sock");
    let ask = r#"{"id":"ABCD1234","project":"walk","text":"probe?","options":["yes","no"],"audience":"human"}"#;
    let asked = on_socket(
        &socket,
        &format!(
            "POST /ask HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n{ask}",
            ask.len()
        ),
    );
    assert_eq!(status_of(&asked), 200, "{asked}");

    let waiter = {
        let socket = socket.clone();
        std::thread::spawn(move || {
            let started = std::time::Instant::now();
            let response = on_socket(
                &socket,
                "GET /wait/ABCD1234?s=10 HTTP/1.1\r\nHost: x\r\n\r\n",
            );
            (response, started.elapsed())
        })
    };
    std::thread::sleep(std::time::Duration::from_millis(300));
    let answered = post_answer(
        &hub,
        TOKEN,
        r#"{"question":"ABCD1234","answer":"yes","by":"slack"}"#,
    );
    assert_eq!(status_of(&answered), 200, "{answered}");
    let (response, took) = waiter.join().unwrap();
    assert_eq!(status_of(&response), 200, "{response}");
    assert_eq!(body_of(&response), "yes");
    assert!(took < std::time::Duration::from_secs(2), "{took:?}");

    // A wait after the answer has it at once.
    let again = on_socket(
        &socket,
        "GET /wait/ABCD1234?s=10 HTTP/1.1\r\nHost: x\r\n\r\n",
    );
    assert_eq!(body_of(&again), "yes");
}

#[test]
fn a_wait_with_no_answer_ends_empty_when_its_time_is_up() {
    let dir = TempDir::new("local-noanswer");
    let _hub = spawn_local(&dir, "testbox");
    let started = std::time::Instant::now();
    let response = on_socket(
        &dir.join("local.sock"),
        "GET /wait/NOPE0000?s=1 HTTP/1.1\r\nHost: x\r\n\r\n",
    );
    assert_eq!(status_of(&response), 204, "{response}");
    assert!(started.elapsed() >= std::time::Duration::from_millis(900));
}

#[test]
fn an_answer_without_the_peer_token_wakes_nobody() {
    let dir = TempDir::new("local-forged");
    let hub = spawn_local(&dir, "testbox");
    let forged = post_answer(
        &hub,
        "wrong",
        r#"{"question":"ABCD1234","answer":"yes","by":"slack"}"#,
    );
    assert_eq!(status_of(&forged), 401);
    let response = on_socket(
        &dir.join("local.sock"),
        "GET /wait/ABCD1234?s=1 HTTP/1.1\r\nHost: x\r\n\r\n",
    );
    assert_eq!(status_of(&response), 204, "{response}");
}

const ASK: &str = r#"{"id":"ABCD1234","project":"walk","text":"probe?","options":["yes","no"],"audience":"human"}"#;

fn ask_on(socket: &Path) {
    let asked = on_socket(
        socket,
        &format!(
            "POST /ask HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n{ASK}",
            ASK.len()
        ),
    );
    assert_eq!(status_of(&asked), 200, "{asked}");
}

/// A mem that writes its argv, one per line, to `log`.
fn logging_mem(dir: &TempDir) -> (std::path::PathBuf, std::path::PathBuf) {
    let log = dir.join("mem.log");
    let bin = dir.join("bin");
    common::fixture_mem(
        &bin,
        &format!("printf '%s\\n' \"$@\" >> '{}'", log.display()),
    );
    (bin, log)
}

#[test]
fn an_answer_to_a_question_asked_here_is_recorded_in_the_store_signed_by_its_surface() {
    let dir = TempDir::new("local-record");
    let (bin, log) = logging_mem(&dir);
    let hub = spawn_local_with(&dir, "testbox", &[&bin], &[]);
    ask_on(&dir.join("local.sock"));
    let answered = post_answer(
        &hub,
        TOKEN,
        r#"{"question":"ABCD1234","answer":"yes","by":"slack"}"#,
    );
    assert_eq!(status_of(&answered), 200, "{answered}");
    common::wait_for("mem answer", std::time::Duration::from_secs(5), || {
        std::fs::read_to_string(&log).is_ok_and(|l| l.lines().count() >= 6)
    });
    let argv: Vec<String> = std::fs::read_to_string(&log)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(argv, ["answer", "--by", "slack", "--", "ABCD1234", "yes"]);
}

/// A cloud session's mem is the mem server's, which refuses an answer; its
/// store record waits for sharing.
#[test]
fn a_hub_whose_mem_is_remote_records_nothing() {
    let dir = TempDir::new("local-remote");
    let (bin, log) = logging_mem(&dir);
    let hub = spawn_local_with(
        &dir,
        "testbox",
        &[&bin],
        &[("MEM_REMOTE", "https://mem.example.test")],
    );
    ask_on(&dir.join("local.sock"));
    post_answer(
        &hub,
        TOKEN,
        r#"{"question":"ABCD1234","answer":"yes","by":"slack"}"#,
    );
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(!log.exists(), "{:?}", std::fs::read_to_string(&log));
}

/// An answer for a question this hub never asked is a peer's to record.
#[test]
fn an_answer_to_a_question_asked_elsewhere_is_not_recorded_here() {
    let dir = TempDir::new("local-elsewhere");
    let (bin, log) = logging_mem(&dir);
    let hub = spawn_local_with(&dir, "testbox", &[&bin], &[]);
    post_answer(
        &hub,
        TOKEN,
        r#"{"question":"ZZZZ9999","answer":"yes","by":"slack"}"#,
    );
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(!log.exists());
}

/// The peer port carries taps from Slack and nothing else, so an answer
/// claiming any other surface is refused rather than recorded under it.
#[test]
fn a_peer_answer_signed_by_anything_but_slack_is_refused() {
    let dir = TempDir::new("local-signed");
    let (bin, log) = logging_mem(&dir);
    let hub = spawn_local_with(&dir, "testbox", &[&bin], &[]);
    ask_on(&dir.join("local.sock"));
    let forged = post_answer(
        &hub,
        TOKEN,
        r#"{"question":"ABCD1234","answer":"yes","by":"saiful"}"#,
    );
    assert_eq!(status_of(&forged), 400, "{forged}");
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(!log.exists());
    let response = on_socket(
        &dir.join("local.sock"),
        "GET /wait/ABCD1234?s=1 HTTP/1.1\r\nHost: x\r\n\r\n",
    );
    assert_eq!(status_of(&response), 204, "{response}");
}

/// A roadmap review starts work once approved: it is Saiful's to answer on the
/// hub, never by a tap carried between hubs.
#[test]
fn a_roadmap_review_cannot_be_answered_over_the_peer_port() {
    let dir = TempDir::new("local-review");
    let (bin, log) = logging_mem(&dir);
    let hub = spawn_local_with(&dir, "testbox", &[&bin], &[]);
    let ask = r#"{"id":"REVW1234","project":"walk","text":"Review the walk roadmap","options":["approve","changes"],"audience":"human"}"#;
    let asked = on_socket(
        &dir.join("local.sock"),
        &format!(
            "POST /ask HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n{ask}",
            ask.len()
        ),
    );
    assert_eq!(status_of(&asked), 200, "{asked}");
    let refused = post_answer(
        &hub,
        TOKEN,
        r#"{"question":"REVW1234","answer":"approve","by":"slack"}"#,
    );
    assert_eq!(status_of(&refused), 403, "{refused}");
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(!log.exists());
    let response = on_socket(
        &dir.join("local.sock"),
        "GET /wait/REVW1234?s=1 HTTP/1.1\r\nHost: x\r\n\r\n",
    );
    assert_eq!(status_of(&response), 204, "{response}");
}

/// Loopback only: tailnet peers come in through `tailscale serve` or
/// userspace Tailscale, never straight off a network the machine has joined.
#[test]
fn the_peer_port_listens_on_loopback_only() {
    let dir = TempDir::new("local-loopback");
    let mut hub = spawn_local(&dir, "testbox");
    let mut socket_line = String::new();
    std::io::BufRead::read_line(&mut hub.stdout, &mut socket_line).unwrap();
    let ours = std::net::TcpStream::connect(("127.0.0.1", hub.port));
    assert!(ours.is_ok());
    let listening = std::fs::read_to_string("/proc/net/tcp").unwrap();
    let port = format!(":{:04X} ", hub.port);
    let rows: Vec<&str> = listening
        .lines()
        .filter(|l| {
            l.split_whitespace()
                .nth(1)
                .is_some_and(|a| a.ends_with(port.trim()))
        })
        .filter(|l| l.split_whitespace().nth(3) == Some("0A"))
        .collect();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(
        rows[0]
            .split_whitespace()
            .nth(1)
            .unwrap()
            .starts_with("0100007F:"),
        "{rows:?}"
    );
}

/// The first answer stands: a second one handed in for the same question,
/// a replayed tap or a forged one, changes nothing.
#[test]
fn a_second_answer_to_an_answered_question_is_refused() {
    let dir = TempDir::new("local-replay");
    let (bin, log) = logging_mem(&dir);
    let hub = spawn_local_with(&dir, "testbox", &[&bin], &[]);
    ask_on(&dir.join("local.sock"));
    let first = post_answer(
        &hub,
        TOKEN,
        r#"{"question":"ABCD1234","answer":"yes","by":"slack"}"#,
    );
    assert_eq!(status_of(&first), 200, "{first}");
    common::wait_for("mem answer", std::time::Duration::from_secs(5), || {
        std::fs::read_to_string(&log).is_ok_and(|l| l.lines().count() >= 6)
    });
    let second = post_answer(
        &hub,
        TOKEN,
        r#"{"question":"ABCD1234","answer":"no","by":"slack"}"#,
    );
    assert_eq!(status_of(&second), 409, "{second}");
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert_eq!(std::fs::read_to_string(&log).unwrap().lines().count(), 6);
    let response = on_socket(
        &dir.join("local.sock"),
        "GET /wait/ABCD1234?s=1 HTTP/1.1\r\nHost: x\r\n\r\n",
    );
    assert_eq!(body_of(&response), "yes");
}
