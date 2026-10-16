//! A cloud session's local hub holds the cloud token, never the device
//! token: it may ask and answer its own questions, and nothing more.

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{Hub, TempDir, body_of, hub_bin, status_of};

const DEVICE: &str = "peer-token-for-tests-0123456789abcdef";
const CLOUD: &str = "cloud-token-for-tests-fedcba9876543210";
/// sha256 of `DEVICE`, as `sha256sum` prints it.
const DEVICE_SHA256: &str = "8ca90bdbfc0b6f6eb1a8994482ec2026cd9677a5e9c82efc84d2bae6eb00c3f3";

const ASK: &str = r#"{"id":"ABCD1234","project":"walk","text":"probe?","options":["yes","no"],"audience":"human"}"#;
const ANSWER: &str = r#"{"question":"ABCD1234","answer":"yes","by":"slack"}"#;

/// A hub named `machine` with someone at it, a mem that logs its argv to
/// `mem.log`, and `env` for its tokens.
fn spawn(dir: &TempDir, machine: &str, env: &[(&str, &str)]) -> Hub {
    let home = dir.join("home");
    std::fs::create_dir_all(home.join("config/qshell")).unwrap();
    std::fs::create_dir_all(home.join("state/qshell")).unwrap();
    std::fs::write(home.join("config/qshell/machine"), format!("{machine}\n")).unwrap();
    let bin = dir.join("bin");
    common::fixture_mem(
        &bin,
        &format!(
            "case \"$*\" in *questions*) echo '{{\"questions\":[]}}';; \
             *) printf '%s\\n' \"$@\" >> '{}';; esac",
            dir.join("mem.log").display()
        ),
    );
    common::fixture_bin(&bin, "pgrep", "exit 0");
    let socket = dir.join("local.sock");
    let mut all = vec![
        ("HUB_LOCAL_SOCKET", socket.to_str().unwrap()),
        ("HUB_HOST", "127.0.0.1"),
    ];
    all.extend_from_slice(env);
    Hub::spawn_env(&home, &[&bin], &["local", "--port", "0"], &all)
}

fn device_hub(dir: &TempDir) -> Hub {
    spawn(
        dir,
        "mini",
        &[("HUB_PEER_TOKEN", DEVICE), ("HUB_CLOUD_TOKEN", CLOUD)],
    )
}

fn cloud_env() -> Vec<(&'static str, &'static str)> {
    vec![
        ("HUB_CLOUD_TOKEN", CLOUD),
        ("HUB_PEER_TOKEN_SHA256", DEVICE_SHA256),
    ]
}

fn ask_on(dir: &TempDir) {
    let mut stream = UnixStream::connect(dir.join("local.sock")).unwrap();
    stream
        .write_all(
            format!(
                "POST /ask HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n{ASK}",
                ASK.len()
            )
            .as_bytes(),
        )
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert_eq!(status_of(&response), 200, "{response}");
}

fn send(hub: &Hub, method: &str, path: &str, token: &str, body: &str) -> String {
    hub.raw(&format!(
        "{method} {path} HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    ))
}

fn mem_log(dir: &TempDir) -> PathBuf {
    dir.join("mem.log")
}

fn recorded(log: &Path) -> Vec<String> {
    common::wait_for("mem answer", Duration::from_secs(5), || {
        std::fs::read_to_string(log).is_ok_and(|l| l.lines().count() >= 6)
    });
    std::fs::read_to_string(log)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect()
}

/// Each request's path and Authorization header, in the order they came.
type Seen = Arc<Mutex<Vec<(String, String)>>>;

/// A listener that answers every request `{"ok":false}` and keeps what
/// it was sent.
fn fake() -> (u16, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let keep = Arc::clone(&seen);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            let path = line.split(' ').nth(1).unwrap_or("").to_string();
            let (mut auth, mut length) = (String::new(), 0);
            loop {
                let mut header = String::new();
                if reader.read_line(&mut header).unwrap_or(0) == 0 {
                    break;
                }
                let header = header.trim_end();
                if header.is_empty() {
                    break;
                }
                if let Some((name, value)) = header.split_once(':') {
                    if name.eq_ignore_ascii_case("authorization") {
                        auth = value.trim().to_string();
                    } else if name.eq_ignore_ascii_case("content-length") {
                        length = value.trim().parse().unwrap_or(0);
                    }
                }
            }
            let mut body = vec![0; length];
            let _ = reader.read_exact(&mut body);
            keep.lock().unwrap().push((path, auth));
            let reply = r#"{"ok":false,"error":"fake"}"#;
            let mut stream = stream;
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    reply.len()
                )
                .as_bytes(),
            );
        }
    });
    (port, seen)
}

#[test]
fn a_device_hub_refuses_an_answer_sent_with_the_cloud_token() {
    let dir = TempDir::new("cloud-device-answer");
    let hub = device_hub(&dir);
    ask_on(&dir);
    let refused = send(&hub, "POST", "/answer", CLOUD, ANSWER);
    assert_eq!(status_of(&refused), 403, "{refused}");
    assert_eq!(body_of(&refused).trim(), "a cloud session may only ask");
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        !mem_log(&dir).exists(),
        "{:?}",
        std::fs::read_to_string(mem_log(&dir))
    );

    let answered = send(&hub, "POST", "/answer", DEVICE, ANSWER);
    assert_eq!(status_of(&answered), 200, "{answered}");
    assert_eq!(
        recorded(&mem_log(&dir)),
        ["answer", "--by", "slack", "--", "ABCD1234", "yes"]
    );
}

#[test]
fn a_device_hub_takes_a_ping_and_a_ring_with_the_cloud_token() {
    let dir = TempDir::new("cloud-device-ask");
    let hub = device_hub(&dir);
    let ping = send(&hub, "GET", "/ping", CLOUD, "");
    assert_eq!(status_of(&ping), 200, "{ping}");
    assert_eq!(body_of(&ping).trim(), "mini");
    let open = r#"{"id":"RING0001","project":"walk","machine":"cloud-1","text":"probe?","options":["yes"],"person":true,"ring":null}"#;
    let rung = send(&hub, "POST", "/ring", CLOUD, open);
    assert_eq!(status_of(&rung), 200, "{rung}");
    assert_eq!(body_of(&rung).trim(), "taken");
}

#[test]
fn a_cloud_hub_takes_an_answer_from_a_device_and_not_from_a_cloud_session() {
    let dir = TempDir::new("cloud-hub-answer");
    let hub = spawn(&dir, "cloud-1", &cloud_env());
    ask_on(&dir);
    let refused = send(&hub, "POST", "/answer", CLOUD, ANSWER);
    assert_eq!(status_of(&refused), 403, "{refused}");
    std::thread::sleep(Duration::from_millis(500));
    assert!(!mem_log(&dir).exists());

    let answered = send(&hub, "POST", "/answer", DEVICE, ANSWER);
    assert_eq!(status_of(&answered), 200, "{answered}");
    assert_eq!(
        recorded(&mem_log(&dir)),
        ["answer", "--by", "slack", "--", "ABCD1234", "yes"]
    );
}

#[test]
fn a_cloud_hub_sends_the_cloud_token_to_its_peers() {
    let dir = TempDir::new("cloud-hub-sends");
    let (port, seen) = fake();
    let peers = format!("127.0.0.1:{port}");
    let mut env = cloud_env();
    env.push(("HUB_PEERS", &peers));
    let _hub = spawn(&dir, "cloud-1", &env);
    common::wait_for("a peer request", Duration::from_secs(5), || {
        !seen.lock().unwrap().is_empty()
    });
    let (path, auth) = seen.lock().unwrap()[0].clone();
    assert_eq!(path, "/route");
    assert_eq!(auth, format!("Bearer {CLOUD}"));
}

#[test]
fn a_cloud_hub_opens_no_slack_socket() {
    let dir = TempDir::new("cloud-hub-slack");
    let (port, seen) = fake();
    let api = format!("http://127.0.0.1:{port}/api");
    let mut env = cloud_env();
    env.extend([
        ("SLACK_BOT_TOKEN", "xoxb-fake"),
        ("SLACK_APP_TOKEN", "xapp-fake"),
        ("SLACK_USER_ID", "U0SAIFUL"),
        ("SLACK_API_URL", api.as_str()),
    ]);
    let _hub = spawn(&dir, "cloud-1", &env);
    // It still posts its own question.
    ask_on(&dir);
    common::wait_for("the question's post", Duration::from_secs(5), || {
        seen.lock()
            .unwrap()
            .iter()
            .any(|(p, _)| p == "/api/chat.postMessage")
    });
    std::thread::sleep(Duration::from_secs(2));
    let paths: Vec<String> = seen
        .lock()
        .unwrap()
        .iter()
        .map(|(p, _)| p.clone())
        .collect();
    assert!(
        !paths.iter().any(|p| p.contains("apps.connections.open")),
        "{paths:?}"
    );
}

/// A cloud session's environment holds the bot token only: the app token
/// opens Socket Mode, which a cloud hub never does.
#[test]
fn a_cloud_hub_posts_its_question_with_the_bot_token_alone() {
    let dir = TempDir::new("cloud-hub-bot-only");
    let (port, seen) = fake();
    let api = format!("http://127.0.0.1:{port}/api");
    let mut env = cloud_env();
    env.extend([
        ("SLACK_BOT_TOKEN", "xoxb-fake"),
        ("SLACK_USER_ID", "U0SAIFUL"),
        ("SLACK_API_URL", api.as_str()),
    ]);
    let _hub = spawn(&dir, "cloud-1", &env);
    ask_on(&dir);
    common::wait_for("the question's post", Duration::from_secs(5), || {
        seen.lock()
            .unwrap()
            .iter()
            .any(|(p, _)| p == "/api/chat.postMessage")
    });
}

#[test]
fn a_hub_with_the_cloud_token_alone_will_not_start() {
    let dir = TempDir::new("cloud-hub-nohash");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let out = Command::new(hub_bin())
        .args(["local", "--port", "0"])
        .env_clear()
        .env("HOME", &home)
        .env("PATH", "/usr/bin:/bin")
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("HUB_LOCAL_SOCKET", dir.join("local.sock"))
        .env("HUB_CLOUD_TOKEN", CLOUD)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(
        out.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("HUB_PEER_TOKEN_SHA256"), "{stderr}");
}
