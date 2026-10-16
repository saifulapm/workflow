//! The Slack client against a fake Slack on localhost: a plain HTTP server
//! for the Web API and a tungstenite server for Socket Mode.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use hub::slack::{Question, Slack, Tap};
use serde::Deserialize;
use serde_json::{Value, json};
use tungstenite::Message;

const BOT: &str = "xoxb-fake-SECRET-BOT";
const APP: &str = "xapp-fake-SECRET-APP";
const SAIFUL: &str = "U0SAIFUL";

struct Call {
    path: String,
    auth: String,
    body: String,
}

struct Fake {
    api: String,
    calls: Arc<Mutex<Vec<Call>>>,
    opens: Arc<AtomicUsize>,
    sockets: Arc<AtomicUsize>,
    acks: Arc<Mutex<Vec<String>>>,
}

/// Starts the fake. Every Socket Mode connection gets `hello`, then `script`.
fn fake(script: Vec<String>) -> Fake {
    let ws = TcpListener::bind("127.0.0.1:0").unwrap();
    let ws_port = ws.local_addr().unwrap().port();
    let sockets = Arc::new(AtomicUsize::new(0));
    let acks = Arc::new(Mutex::new(Vec::new()));
    {
        let sockets = Arc::clone(&sockets);
        let acks = Arc::clone(&acks);
        thread::spawn(move || {
            for stream in ws.incoming() {
                let Ok(stream) = stream else { continue };
                sockets.fetch_add(1, Ordering::SeqCst);
                let script = script.clone();
                let acks = Arc::clone(&acks);
                thread::spawn(move || {
                    let Ok(mut socket) = tungstenite::accept(stream) else {
                        return;
                    };
                    let hello = r#"{"type":"hello","num_connections":1}"#;
                    if socket.send(Message::text(hello)).is_err() {
                        return;
                    }
                    for frame in script {
                        if socket.send(Message::text(frame)).is_err() {
                            return;
                        }
                    }
                    while let Ok(message) = socket.read() {
                        if let Message::Text(text) = message {
                            acks.lock().unwrap().push(text.to_string());
                        }
                    }
                });
            }
        });
    }

    let http = TcpListener::bind("127.0.0.1:0").unwrap();
    let api = format!("http://{}/api", http.local_addr().unwrap());
    let calls = Arc::new(Mutex::new(Vec::new()));
    let opens = Arc::new(AtomicUsize::new(0));
    {
        let calls = Arc::clone(&calls);
        let opens = Arc::clone(&opens);
        thread::spawn(move || {
            for stream in http.incoming() {
                let Ok(stream) = stream else { continue };
                serve(stream, ws_port, &calls, &opens);
            }
        });
    }
    Fake {
        api,
        calls,
        opens,
        sockets,
        acks,
    }
}

fn serve(stream: TcpStream, ws_port: u16, calls: &Mutex<Vec<Call>>, opens: &AtomicUsize) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let path = line.split(' ').nth(1).unwrap_or("").to_string();
    let mut auth = String::new();
    let mut length = 0;
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).unwrap();
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        let (name, value) = header.split_once(':').unwrap();
        match name.to_ascii_lowercase().as_str() {
            "authorization" => auth = value.trim().to_string(),
            "content-length" => length = value.trim().parse().unwrap(),
            _ => {}
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
            opens.fetch_add(1, Ordering::SeqCst);
            format!(r#"{{"ok":true,"url":"ws://127.0.0.1:{ws_port}/link/?ticket=t1"}}"#)
        }
        _ => r#"{"ok":false,"error":"unknown_method"}"#.to_string(),
    };
    calls.lock().unwrap().push(Call {
        path,
        auth,
        body: String::from_utf8(body).unwrap(),
    });
    let mut stream = stream;
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
        reply.len()
    );
}

/// A client on the fake, whose log lines land in the returned Vec.
fn client(api: &str) -> (Slack, Arc<Mutex<Vec<String>>>) {
    let lines = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&lines);
    let slack = Slack::new(BOT.into(), APP.into(), SAIFUL.into(), api.into())
        .with_log(move |line| sink.lock().unwrap().push(line.to_string()));
    (slack, lines)
}

fn tap_envelope(envelope: &str, user: &str) -> String {
    json!({
        "envelope_id": envelope,
        "type": "interactive",
        "accepts_response_payload": false,
        "payload": {
            "type": "block_actions",
            "user": {"id": user},
            "channel": {"id": "D0DM"},
            "container": {"type": "message", "channel_id": "D0DM", "message_ts": "1700000000.000100"},
            "message": {"ts": "1700000000.000100"},
            "actions": [{
                "action_id": "0",
                "type": "button",
                "value": r#"{"m":"nuc","q":"Q1","o":"yes"}"#
            }]
        }
    })
    .to_string()
}

fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "timed out: {what}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn socket(slack: Slack) -> mpsc::Receiver<Tap> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        slack.run_socket(move |tap| {
            let _ = tx.send(tap);
        })
    });
    rx
}

#[derive(Deserialize, Debug, PartialEq)]
struct ButtonValue {
    m: String,
    q: String,
    o: String,
}

const LONG: &str = "keep the old store running beside the new one until every replica has caught up and been checked";

#[test]
fn a_question_posts_to_saifuls_dm_with_one_button_per_option() {
    let fake = fake(vec![]);
    let (slack, _) = client(&fake.api);
    let question = Question {
        id: "Q1".into(),
        project: "walk".into(),
        machine: "nuc".into(),
        text: "probe?".into(),
        options: vec!["yes".into(), "no".into(), LONG.into()],
    };

    let posted = slack.post_question(&question).unwrap();

    assert_eq!(posted.channel, "D0DM");
    assert_eq!(posted.ts, "1700000000.000100");
    let calls = fake.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].path, "/api/chat.postMessage");
    assert_eq!(calls[0].auth, "Bearer xoxb-fake-SECRET-BOT");
    let body: Value = serde_json::from_str(&calls[0].body).unwrap();
    assert_eq!(body["channel"], "U0SAIFUL");
    let blocks = body["blocks"].as_array().unwrap();
    let section = blocks[0]["text"]["text"].as_str().unwrap();
    for part in ["walk", "nuc", "probe?"] {
        assert!(section.contains(part), "{section:?} lacks {part}");
    }
    let buttons = blocks[1]["elements"].as_array().unwrap();
    let values: Vec<ButtonValue> = buttons
        .iter()
        .map(|b| serde_json::from_str(b["value"].as_str().unwrap()).unwrap())
        .collect();
    let want = |o: &str| ButtonValue {
        m: "nuc".into(),
        q: "Q1".into(),
        o: o.into(),
    };
    assert_eq!(values, vec![want("yes"), want("no"), want(LONG)]);
    assert_eq!(buttons[0]["text"]["text"], "yes");
    assert_eq!(
        buttons[2]["text"]["text"].as_str().unwrap().chars().count(),
        75
    );
}

#[test]
fn an_answered_question_loses_its_buttons() {
    let fake = fake(vec![]);
    let (slack, _) = client(&fake.api);

    slack
        .mark_answered("D0DM", "1700000000.000100", "yes", "probe?")
        .unwrap();

    let calls = fake.calls.lock().unwrap();
    assert_eq!(calls[0].path, "/api/chat.update");
    assert_eq!(calls[0].auth, "Bearer xoxb-fake-SECRET-BOT");
    let body: Value = serde_json::from_str(&calls[0].body).unwrap();
    assert_eq!(body["channel"], "D0DM");
    assert_eq!(body["ts"], "1700000000.000100");
    assert!(body["text"].as_str().unwrap().contains("yes"));
    let blocks = body["blocks"].as_array().unwrap();
    assert!(blocks.iter().all(|b| b["type"] != "actions"), "{blocks:?}");
}

#[test]
fn saifuls_tap_reaches_on_tap_and_is_acked() {
    let fake = fake(vec![tap_envelope("env-1", SAIFUL)]);
    let (slack, lines) = client(&fake.api);

    let taps = socket(slack);

    let tap = taps.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(tap.machine, "nuc");
    assert_eq!(tap.question, "Q1");
    assert_eq!(tap.answer, "yes");
    assert_eq!(tap.channel, "D0DM");
    assert_eq!(tap.ts, "1700000000.000100");
    let calls = fake.calls.lock().unwrap();
    assert_eq!(calls[0].path, "/api/apps.connections.open");
    assert_eq!(calls[0].auth, "Bearer xapp-fake-SECRET-APP");
    drop(calls);
    wait_for("the ack", || !fake.acks.lock().unwrap().is_empty());
    let ack: Value = serde_json::from_str(&fake.acks.lock().unwrap()[0]).unwrap();
    assert_eq!(ack, json!({"envelope_id": "env-1"}));
    assert_tokenless(&lines.lock().unwrap());
}

#[test]
fn a_tap_by_anyone_else_is_acked_refused_and_logged() {
    let fake = fake(vec![tap_envelope("env-2", "U0OTHER")]);
    let (slack, lines) = client(&fake.api);

    let taps = socket(slack);

    wait_for("the ack", || !fake.acks.lock().unwrap().is_empty());
    let ack: Value = serde_json::from_str(&fake.acks.lock().unwrap()[0]).unwrap();
    assert_eq!(ack, json!({"envelope_id": "env-2"}));
    wait_for("the refusal", || !lines.lock().unwrap().is_empty());
    assert_eq!(
        *lines.lock().unwrap(),
        vec!["slack: refused a tap by U0OTHER on question Q1".to_string()]
    );
    assert!(taps.recv_timeout(Duration::from_millis(300)).is_err());
}

#[test]
fn disconnect_opens_a_new_connection() {
    let disconnect = r#"{"type":"disconnect","reason":"refresh_requested"}"#.to_string();
    let fake = fake(vec![disconnect]);
    let (slack, _) = client(&fake.api);

    let _taps = socket(slack);

    wait_for("a second open", || fake.opens.load(Ordering::SeqCst) >= 2);
    wait_for("a second socket", || {
        fake.sockets.load(Ordering::SeqCst) >= 2
    });
}

#[test]
fn no_token_reaches_a_log_line_an_error_or_debug() {
    // The discard port, which nothing listens on: every call fails, and the
    // socket loop logs. A port bound and dropped here could be another test's
    // listener by the time the call is made.
    let (slack, lines) = client("http://127.0.0.1:9/api");
    let question = Question {
        id: "Q1".into(),
        project: "walk".into(),
        machine: "nuc".into(),
        text: "probe?".into(),
        options: vec!["yes".into()],
    };
    let mut seen = vec![format!("{slack:?}")];
    let error = slack.post_question(&question).unwrap_err();
    seen.push(format!("{error:#}"));
    seen.push(format!("{error:?}"));
    let error = slack
        .mark_answered("D0DM", "1", "yes", "probe?")
        .unwrap_err();
    seen.push(format!("{error:#}"));

    let _taps = socket(slack);
    wait_for("a logged socket error", || {
        !lines.lock().unwrap().is_empty()
    });
    seen.extend(lines.lock().unwrap().iter().cloned());

    assert!(seen[0].contains("U0SAIFUL"), "{:?}", seen[0]);
    assert_tokenless(&seen);
}

fn assert_tokenless(seen: &[String]) {
    for text in seen {
        assert!(!text.contains(BOT), "bot token in {text:?}");
        assert!(!text.contains(APP), "app token in {text:?}");
        assert!(!text.contains("SECRET"), "a token in {text:?}");
    }
}

/// A connection that goes silent, pings unanswered, is dropped and opened
/// again, rather than read from for good.
#[test]
fn a_silent_connection_is_opened_again() {
    let ws = TcpListener::bind("127.0.0.1:0").unwrap();
    let ws_port = ws.local_addr().unwrap().port();
    let sockets = Arc::new(AtomicUsize::new(0));
    {
        let sockets = Arc::clone(&sockets);
        thread::spawn(move || {
            let mut held = Vec::new();
            for stream in ws.incoming() {
                let Ok(stream) = stream else { continue };
                sockets.fetch_add(1, Ordering::SeqCst);
                let Ok(mut socket) = tungstenite::accept(stream) else {
                    continue;
                };
                let _ = socket.send(Message::text(r#"{"type":"hello"}"#));
                // Held open and never read, so no ping is ever answered.
                held.push(socket);
            }
        });
    }
    let http = TcpListener::bind("127.0.0.1:0").unwrap();
    let api = format!("http://{}/api", http.local_addr().unwrap());
    thread::spawn(move || {
        for stream in http.incoming() {
            let Ok(stream) = stream else { continue };
            let calls = Mutex::new(Vec::new());
            serve(stream, ws_port, &calls, &AtomicUsize::new(0));
        }
    });
    let (slack, _) = client(&api);
    let _taps = socket(slack.with_quiet(Duration::from_millis(600)));
    wait_for("a second socket", || sockets.load(Ordering::SeqCst) >= 2);
}

/// A cloud VM reaches the internet only through an HTTP proxy, which
/// tungstenite does not speak: Socket Mode tunnels through it with CONNECT.
#[test]
fn socket_mode_goes_through_the_proxy_when_one_is_named() {
    let fake = fake(vec![tap_envelope("env-p", SAIFUL)]);
    let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy_url = format!("http://{}", proxy.local_addr().unwrap());
    let asked = Arc::new(Mutex::new(Vec::new()));
    {
        let asked = Arc::clone(&asked);
        thread::spawn(move || {
            for client in proxy.incoming() {
                let Ok(client) = client else { continue };
                let asked = Arc::clone(&asked);
                thread::spawn(move || {
                    let mut reader = BufReader::new(client.try_clone().unwrap());
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    loop {
                        let mut header = String::new();
                        reader.read_line(&mut header).unwrap();
                        if header == "\r\n" {
                            break;
                        }
                    }
                    let target = line.split(' ').nth(1).unwrap().to_string();
                    asked.lock().unwrap().push(line.trim().to_string());
                    let upstream = TcpStream::connect(&target).unwrap();
                    let mut client = client;
                    client
                        .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                        .unwrap();
                    let mut up = upstream.try_clone().unwrap();
                    let mut down = client.try_clone().unwrap();
                    thread::spawn(move || {
                        let _ = std::io::copy(&mut reader, &mut up);
                    });
                    let mut upstream = upstream;
                    let _ = std::io::copy(&mut upstream, &mut down);
                });
            }
        });
    }
    let (slack, _) = client(&fake.api);
    let taps = socket(slack.with_proxy(Some(proxy_url)));
    let tap = taps.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(tap.answer, "yes");
    let asked = asked.lock().unwrap();
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert!(asked[0].starts_with("CONNECT 127.0.0.1:"), "{asked:?}");
}

/// Slack refuses an actions block with no elements, so a question without
/// options goes as the text alone, to be answered in its thread.
#[test]
fn a_question_without_options_posts_without_buttons() {
    let fake = fake(Vec::new());
    let (slack, _) = client(&fake.api);
    slack
        .post_question(&Question {
            id: "Q2".into(),
            project: "walk".into(),
            machine: "nuc".into(),
            text: "which name?".into(),
            options: Vec::new(),
        })
        .unwrap();
    let calls = fake.calls.lock().unwrap();
    let posted = calls
        .iter()
        .find(|c| c.path == "/api/chat.postMessage")
        .unwrap();
    let body: Value = serde_json::from_str(&posted.body).unwrap();
    let blocks = body["blocks"].as_array().unwrap();
    assert!(blocks.iter().all(|b| b["type"] != "actions"), "{body}");
    assert!(body["text"].as_str().unwrap().contains("which name?"));
}
