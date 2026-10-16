//! Two local hubs and Slack: a question asked on one is posted to Slack, a
//! tap on its button reaches whichever local hub holds the Socket Mode
//! connection Slack picked, and that hub hands the answer to the hub that
//! asked, whose waiting mem wakes.

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use common::{Hub, TempDir, body_of, status_of};
use serde_json::{Value, json};
use tungstenite::Message;

const TOKEN: &str = "peer-token-for-tests-0123456789abcdef";
const BOT: &str = "xoxb-fake-SECRET-BOT";
const APP: &str = "xapp-fake-SECRET-APP";
const SAIFUL: &str = "U0SAIFUL";

/// A fake Slack: the Web API on one port and Socket Mode on another. Frames
/// sent down `push` go to the newest Socket Mode connection.
struct Fake {
    api: String,
    calls: Arc<Mutex<Vec<(String, String)>>>,
    push: mpsc::Sender<String>,
    acks: Arc<Mutex<Vec<String>>>,
}

fn fake() -> Fake {
    let ws = TcpListener::bind("127.0.0.1:0").unwrap();
    let ws_port = ws.local_addr().unwrap().port();
    let (push, frames) = mpsc::channel::<String>();
    let acks = Arc::new(Mutex::new(Vec::new()));
    {
        let acks = Arc::clone(&acks);
        thread::spawn(move || {
            let (stream, _) = ws.accept().unwrap();
            let mut socket = tungstenite::accept(stream).unwrap();
            socket
                .send(Message::text(r#"{"type":"hello","num_connections":1}"#))
                .unwrap();
            socket
                .get_mut()
                .set_read_timeout(Some(Duration::from_millis(50)))
                .unwrap();
            loop {
                if let Ok(frame) = frames.try_recv() {
                    socket.send(Message::text(frame)).unwrap();
                }
                match socket.read() {
                    Ok(Message::Text(text)) => acks.lock().unwrap().push(text.to_string()),
                    Ok(_) => {}
                    Err(tungstenite::Error::Io(e))
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::TimedOut => {}
                    Err(_) => return,
                }
            }
        });
    }
    let http = TcpListener::bind("127.0.0.1:0").unwrap();
    let api = format!("http://{}/api", http.local_addr().unwrap());
    let calls = Arc::new(Mutex::new(Vec::new()));
    {
        let calls = Arc::clone(&calls);
        thread::spawn(move || {
            for stream in http.incoming() {
                let Ok(stream) = stream else { continue };
                serve(stream, ws_port, &calls);
            }
        });
    }
    Fake {
        api,
        calls,
        push,
        acks,
    }
}

fn serve(stream: TcpStream, ws_port: u16, calls: &Mutex<Vec<(String, String)>>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let path = line.split(' ').nth(1).unwrap_or("").to_string();
    let mut length = 0;
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).unwrap();
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap();
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).unwrap();
    let reply = match path.as_str() {
        "/api/chat.postMessage" => {
            r#"{"ok":true,"channel":"D0DM","ts":"1700000000.000100"}"#.to_string()
        }
        "/api/chat.update" => r#"{"ok":true}"#.to_string(),
        "/api/apps.connections.open" => {
            format!(r#"{{"ok":true,"url":"ws://127.0.0.1:{ws_port}/link/?ticket=t1"}}"#)
        }
        _ => r#"{"ok":false,"error":"unknown_method"}"#.to_string(),
    };
    calls
        .lock()
        .unwrap()
        .push((path, String::from_utf8(body).unwrap()));
    let mut stream = stream;
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
        reply.len()
    );
}

impl Fake {
    fn calls_to(&self, method: &str) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(path, _)| path == &format!("/api/{method}"))
            .map(|(_, body)| body.clone())
            .collect()
    }

    /// A block_actions envelope: `user` tapped the button whose value is
    /// `value`.
    fn tap(&self, user: &str, value: &str) {
        let payload = json!({
            "type": "block_actions",
            "user": {"id": user},
            "container": {"channel_id": "D0DM", "message_ts": "1700000000.000100"},
            "actions": [{"action_id": "answer:0", "value": value}],
        });
        let envelope = json!({
            "envelope_id": "env-1",
            "type": "interactive",
            "payload": payload,
        });
        self.push.send(envelope.to_string()).unwrap();
    }
}

fn spawn_local(dir: &TempDir, machine: &str, slack: &Fake, log: &Path) -> Hub {
    let home = dir.join("home");
    std::fs::create_dir_all(home.join("config/qshell")).unwrap();
    std::fs::write(home.join("config/qshell/machine"), machine).unwrap();
    let socket = dir.join("local.sock");
    let file = std::fs::File::create(log).unwrap();
    Hub::spawn_with_stderr(
        &home,
        &[],
        &["local", "--port", "0"],
        &[
            ("HUB_PEER_TOKEN", TOKEN),
            ("HUB_LOCAL_SOCKET", socket.to_str().unwrap()),
            ("HUB_HOST", "127.0.0.1"),
            ("SLACK_BOT_TOKEN", BOT),
            ("SLACK_APP_TOKEN", APP),
            ("SLACK_USER_ID", SAIFUL),
            ("SLACK_API_URL", &slack.api),
        ],
        file,
    )
}

fn on_socket(socket: &Path, request: &str) -> String {
    let mut stream = UnixStream::connect(socket).unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

fn ask(socket: &Path, audience: &str) {
    let body = json!({
        "id": "ABCD1234",
        "project": "walk",
        "text": "probe?",
        "options": ["yes", "no"],
        "audience": audience,
    })
    .to_string();
    let response = on_socket(
        socket,
        &format!(
            "POST /ask HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ),
    );
    assert_eq!(status_of(&response), 200, "{response}");
}

fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !f() {
        assert!(Instant::now() < deadline, "timed out: {what}");
        thread::sleep(Duration::from_millis(20));
    }
}

/// The value of the posted message's button for `option`.
fn button_value(posted: &str, option: &str) -> String {
    let message: Value = serde_json::from_str(posted).unwrap();
    for block in message["blocks"].as_array().unwrap() {
        for element in block["elements"].as_array().into_iter().flatten() {
            let value = element["value"].as_str().unwrap_or_default();
            if let Ok(v) = serde_json::from_str::<Value>(value)
                && v["o"] == option
            {
                return value.to_string();
            }
        }
    }
    panic!("no button for {option} in {posted}");
}

struct Pair {
    asker_dir: TempDir,
    _taker_dir: TempDir,
    asker_slack: Fake,
    asker_log: std::path::PathBuf,
    _asker: Hub,
    taker_slack: Fake,
    taker_log: std::path::PathBuf,
    _taker: Hub,
}

fn pair(tag: &str) -> Pair {
    let asker_dir = TempDir::new(&format!("{tag}-asker"));
    let taker_dir = TempDir::new(&format!("{tag}-taker"));
    let asker_slack = fake();
    let taker_slack = fake();
    let asker_log = asker_dir.join("stderr.log");
    let taker_log = taker_dir.join("stderr.log");
    let asker = spawn_local(&asker_dir, "cloudbox", &asker_slack, &asker_log);
    let taker = spawn_local(&taker_dir, "mini", &taker_slack, &taker_log);
    Pair {
        asker_dir,
        _taker_dir: taker_dir,
        asker_slack,
        asker_log,
        _asker: asker,
        taker_slack,
        taker_log,
        _taker: taker,
    }
}

#[test]
fn a_tap_on_another_machine_wakes_the_wait_on_the_asking_one() {
    let p = pair("ls-tap");
    let socket = p.asker_dir.join("local.sock");
    ask(&socket, "human");

    wait_until("the question posted", || {
        !p.asker_slack.calls_to("chat.postMessage").is_empty()
    });
    let posted = &p.asker_slack.calls_to("chat.postMessage")[0];
    let value = button_value(posted, "yes");
    let carried: Value = serde_json::from_str(&value).unwrap();
    assert!(
        carried["m"].as_str().unwrap().starts_with("127.0.0.1:"),
        "the asker names itself with its port: {value}"
    );
    assert_eq!(carried["q"], "ABCD1234");

    let waiter = {
        let socket = socket.clone();
        thread::spawn(move || {
            let started = Instant::now();
            let response = on_socket(
                &socket,
                "GET /wait/ABCD1234?s=10 HTTP/1.1\r\nHost: x\r\n\r\n",
            );
            (response, started.elapsed())
        })
    };
    thread::sleep(Duration::from_millis(200));
    // Slack hands the tap to the other machine's connection.
    p.taker_slack.tap(SAIFUL, &value);
    let (response, took) = waiter.join().unwrap();
    assert_eq!(status_of(&response), 200, "{response}");
    assert_eq!(body_of(&response), "yes");
    assert!(took < Duration::from_secs(2), "{took:?}");

    wait_until("the envelope acked", || {
        p.taker_slack
            .acks
            .lock()
            .unwrap()
            .iter()
            .any(|a| a.contains("env-1"))
    });
    wait_until("the message marked answered", || {
        !p.taker_slack.calls_to("chat.update").is_empty()
    });
    let logs = std::fs::read_to_string(&p.asker_log).unwrap()
        + &std::fs::read_to_string(&p.taker_log).unwrap();
    assert!(
        !logs.contains(BOT) && !logs.contains(APP) && !logs.contains(TOKEN),
        "{logs}"
    );
}

#[test]
fn a_tap_by_anyone_but_saiful_answers_nothing_and_is_logged() {
    let p = pair("ls-refuse");
    let socket = p.asker_dir.join("local.sock");
    ask(&socket, "human");
    wait_until("the question posted", || {
        !p.asker_slack.calls_to("chat.postMessage").is_empty()
    });
    let value = button_value(&p.asker_slack.calls_to("chat.postMessage")[0], "yes");

    p.taker_slack.tap("U0SOMEONE", &value);
    wait_until("the refusal logged", || {
        std::fs::read_to_string(&p.taker_log)
            .is_ok_and(|l| l.contains("refused a tap by U0SOMEONE"))
    });
    let response = on_socket(
        &socket,
        "GET /wait/ABCD1234?s=1 HTTP/1.1\r\nHost: x\r\n\r\n",
    );
    assert_eq!(status_of(&response), 204, "{response}");
    assert!(p.taker_slack.calls_to("chat.update").is_empty());
}

#[test]
fn an_orchestrators_question_never_reaches_slack() {
    let p = pair("ls-orch");
    ask(&p.asker_dir.join("local.sock"), "orchestrator");
    thread::sleep(Duration::from_millis(500));
    assert!(p.asker_slack.calls_to("chat.postMessage").is_empty());
}
