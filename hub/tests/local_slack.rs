//! Two local hubs and Slack: a question asked on one is posted to Slack, a
//! tap on its button reaches whichever local hub holds the Socket Mode
//! connection Slack picked, and that hub hands the answer to the hub that
//! asked, whose waiting mem wakes.

mod common;

use std::collections::HashMap;
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
const PHOTO: &[u8] = b"\x89PNG\r\n\x1a\nfake photo bytes";

/// A fake Slack: the Web API on one port and Socket Mode on another. Frames
/// sent down `push` go to the newest Socket Mode connection.
struct Fake {
    api: String,
    /// Where the fake serves a file: `http://127.0.0.1:<port>`.
    origin: String,
    calls: Arc<Mutex<Vec<(String, String)>>>,
    push: mpsc::Sender<String>,
    acks: Arc<Mutex<Vec<String>>>,
    dm: Arc<Mutex<Dm>>,
}

/// Saiful's DM as Slack keeps it, shared by the fakes of a pair: each hub
/// holds a connection of its own, but both read one conversation.
#[derive(Default)]
struct Dm {
    /// A parent has no `thread_ts`; a reply does.
    messages: Vec<Value>,
    files: HashMap<String, Vec<u8>>,
}

fn fake() -> Fake {
    fake_on(Arc::default())
}

fn fake_on(dm: Arc<Mutex<Dm>>) -> Fake {
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
    let origin = format!("http://{}", http.local_addr().unwrap());
    let api = format!("{origin}/api");
    let calls = Arc::new(Mutex::new(Vec::new()));
    {
        let calls = Arc::clone(&calls);
        let dm = Arc::clone(&dm);
        thread::spawn(move || {
            for stream in http.incoming() {
                let Ok(stream) = stream else { continue };
                serve(stream, ws_port, &calls, &dm);
            }
        });
    }
    Fake {
        api,
        origin,
        calls,
        push,
        acks,
        dm,
    }
}

/// `a=b&c=d`, as a form body carries a read method's arguments.
fn form(body: &str) -> HashMap<String, String> {
    body.split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// A message as a read method returns it: with its metadata only when asked.
fn as_read(message: &Value, args: &HashMap<String, String>) -> Value {
    let mut message = message.clone();
    if args.get("include_all_metadata").map(String::as_str) != Some("true") {
        message.as_object_mut().unwrap().remove("metadata");
    }
    message
}

fn serve(stream: TcpStream, ws_port: u16, calls: &Mutex<Vec<(String, String)>>, dm: &Mutex<Dm>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let path = line.split(' ').nth(1).unwrap_or("").to_string();
    let mut length = 0;
    let mut auth = String::new();
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).unwrap();
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                length = value.trim().parse().unwrap();
            } else if name.eq_ignore_ascii_case("authorization") {
                auth = value.trim().to_string();
            }
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).unwrap();
    let text = String::from_utf8(body).unwrap();
    let args = form(&text);
    let arg = |name: &str| args.get(name).map(String::as_str);
    let mut kind = "application/json";
    let reply: Vec<u8> = match path.as_str() {
        "/api/chat.postMessage" => {
            let posted: Value = serde_json::from_str(&text).unwrap();
            dm.lock().unwrap().messages.push(json!({
                "type": "message",
                "bot_id": "B0BOT",
                "ts": "1700000000.000100",
                "text": posted["text"],
                "blocks": posted["blocks"],
                "metadata": posted["metadata"],
            }));
            br#"{"ok":true,"channel":"D0DM","ts":"1700000000.000100"}"#.to_vec()
        }
        "/api/chat.update" => br#"{"ok":true}"#.to_vec(),
        "/api/apps.connections.open" => {
            format!(r#"{{"ok":true,"url":"ws://127.0.0.1:{ws_port}/link/?ticket=t1"}}"#)
                .into_bytes()
        }
        "/api/conversations.open" if arg("users") == Some(SAIFUL) => {
            br#"{"ok":true,"channel":{"id":"D0DM"}}"#.to_vec()
        }
        "/api/conversations.history" if arg("channel") == Some("D0DM") => {
            let dm = dm.lock().unwrap();
            let parents: Vec<Value> = dm
                .messages
                .iter()
                .filter(|m| m.get("thread_ts").is_none())
                .map(|m| {
                    let mut m = as_read(m, &args);
                    let replies = dm
                        .messages
                        .iter()
                        .filter(|r| r["thread_ts"] == m["ts"])
                        .count();
                    if replies > 0 {
                        m["reply_count"] = json!(replies);
                        m["thread_ts"] = m["ts"].clone();
                    }
                    m
                })
                .rev()
                .collect();
            json!({"ok": true, "messages": parents})
                .to_string()
                .into_bytes()
        }
        "/api/conversations.replies" if arg("channel") == Some("D0DM") => {
            let ts = arg("ts").unwrap_or_default();
            let limit = arg("limit").and_then(|l| l.parse().ok()).unwrap_or(1000);
            let dm = dm.lock().unwrap();
            let thread: Vec<Value> = dm
                .messages
                .iter()
                .filter(|m| m["ts"] == ts || m["thread_ts"] == ts)
                .take(limit)
                .map(|m| as_read(m, &args))
                .collect();
            json!({"ok": true, "messages": thread})
                .to_string()
                .into_bytes()
        }
        file if file.starts_with("/files/") => {
            if auth == format!("Bearer {BOT}") {
                kind = "application/octet-stream";
                dm.lock().unwrap().files[file].clone()
            } else {
                b"not without the bot token".to_vec()
            }
        }
        _ => br#"{"ok":false,"error":"unknown_method"}"#.to_vec(),
    };
    calls.lock().unwrap().push((path, text));
    let mut stream = stream;
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        reply.len()
    );
    let _ = stream.write_all(&reply);
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

    /// An events_api envelope carrying one DM message event.
    fn event(&self, event: Value) {
        let envelope = json!({
            "envelope_id": "env-2",
            "type": "events_api",
            "payload": {"type": "event_callback", "event": event},
        });
        self.push.send(envelope.to_string()).unwrap();
    }

    /// A file only the bot token can fetch, and its private URL.
    fn file(&self, name: &str, bytes: &[u8]) -> String {
        let path = format!("/files/{name}");
        self.dm
            .lock()
            .unwrap()
            .files
            .insert(path.clone(), bytes.to_vec());
        format!("{}{path}", self.origin)
    }
}

/// `user`'s reply in the thread of the posted question.
fn reply_by(user: &str, text: &str) -> Value {
    json!({
        "type": "message",
        "channel": "D0DM",
        "user": user,
        "text": text,
        "ts": "1700000001.000200",
        "thread_ts": "1700000000.000100",
    })
}

/// `event` with one PNG attached, as a `file_share` message carries it.
fn with_photo(mut event: Value, url: &str) -> Value {
    event["subtype"] = json!("file_share");
    event["files"] = json!([{
        "mimetype": "image/png",
        "url_private": url,
        "url_private_download": url,
    }]);
    event
}

/// A mem that copies the file it is handed, which the hub removes once mem
/// returns (a screenshot to `shot.bin`, evidence to `evidence.bin`), then
/// logs its argv as one line to `mem.log`, so a test that sees the line finds
/// the file.
fn recording_mem(dir: &TempDir) -> std::path::PathBuf {
    let bin = dir.join("bin");
    common::fixture_mem(
        &bin,
        &format!(
            "case \"$*\" in *questions*) echo '{{\"questions\":[]}}'; exit 0;; esac\n\
             prev=''\n\
             for a in \"$@\"; do\n\
             if [ \"$prev\" = --screenshot ]; then cp \"$a\" '{d}/shot.bin'; fi\n\
             prev=\"$a\"\n\
             done\n\
             all=\"$*\"; [ \"${{1#--project=}}\" != \"$1\" ] && shift\n\
             if [ \"$1 $2\" = 'evidence add' ]; then cp \"$5\" '{d}/evidence.bin'; fi\n\
             printf '%s\\n' \"$all\" >> '{d}/mem.log'\n\
             exit 0",
            d = dir.path().display()
        ),
    );
    bin
}

/// What the recording mem was asked, one argv a line.
fn mem_log(dir: &TempDir) -> Vec<String> {
    std::fs::read_to_string(dir.join("mem.log"))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

fn spawn_local(dir: &TempDir, machine: &str, slack: &Fake, log: &Path) -> Hub {
    spawn_local_on(dir, machine, slack, log, 0)
}

fn spawn_local_on(dir: &TempDir, machine: &str, slack: &Fake, log: &Path, port: u16) -> Hub {
    let bin = recording_mem(dir);
    let home = dir.join("home");
    std::fs::create_dir_all(home.join("config/qshell")).unwrap();
    std::fs::write(home.join("config/qshell/machine"), machine).unwrap();
    let socket = dir.join("local.sock");
    let file = std::fs::File::create(log).unwrap();
    let port = port.to_string();
    Hub::spawn_with_stderr(
        &home,
        &[&bin],
        &["local", "--port", &port],
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
    ask_with(socket, audience, &["yes", "no"]);
}

fn ask_with(socket: &Path, audience: &str, options: &[&str]) {
    let body = json!({
        "id": "ABCD1234",
        "project": "walk",
        "text": "probe?",
        "options": options,
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

/// A `GET /wait/ABCD1234` held on `socket` from now: its response and how
/// long it took.
fn waiting(socket: &Path, hold: u64) -> thread::JoinHandle<(String, Duration)> {
    let socket = socket.to_path_buf();
    thread::spawn(move || {
        let started = Instant::now();
        let response = on_socket(
            &socket,
            &format!("GET /wait/ABCD1234?s={hold} HTTP/1.1\r\nHost: x\r\n\r\n"),
        );
        (response, started.elapsed())
    })
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
    // Two connections to one workspace: both read the same DM.
    let asker_slack = fake();
    let taker_slack = fake_on(Arc::clone(&asker_slack.dm));
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

/// Asks ABCD1234 on the asker and waits for its Slack post.
fn asked(p: &Pair, options: &[&str]) -> String {
    ask_with(&p.asker_dir.join("local.sock"), "human", options);
    wait_until("the question posted", || {
        !p.asker_slack.calls_to("chat.postMessage").is_empty()
    });
    p.asker_slack.calls_to("chat.postMessage")[0].clone()
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

    let waiter = waiting(&socket, 10);
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

#[test]
fn a_roadmap_review_never_reaches_slack() {
    let p = pair("ls-review");
    let body = json!({
        "id": "REVW1234",
        "project": "walk",
        "text": "Review the walk roadmap",
        "options": ["approve", "changes"],
        "audience": "human",
    })
    .to_string();
    let response = on_socket(
        &p.asker_dir.join("local.sock"),
        &format!(
            "POST /ask HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ),
    );
    assert_eq!(status_of(&response), 200, "{response}");
    thread::sleep(Duration::from_millis(500));
    assert!(p.asker_slack.calls_to("chat.postMessage").is_empty());
}

/// A tap on a question already answered changes nothing, but the message
/// still loses its buttons and shows the answer that stands.
#[test]
fn a_tap_on_an_answered_question_clears_its_buttons_with_the_standing_answer() {
    let p = pair("ls-late");
    ask(&p.asker_dir.join("local.sock"), "human");
    wait_until("the question posted", || {
        !p.asker_slack.calls_to("chat.postMessage").is_empty()
    });
    let posted = p.asker_slack.calls_to("chat.postMessage")[0].clone();
    p.taker_slack.tap(SAIFUL, &button_value(&posted, "yes"));
    wait_until("the first update", || {
        p.taker_slack.calls_to("chat.update").len() == 1
    });
    p.taker_slack.tap(SAIFUL, &button_value(&posted, "no"));
    wait_until("the second update", || {
        p.taker_slack.calls_to("chat.update").len() == 2
    });
    let second = &p.taker_slack.calls_to("chat.update")[1];
    assert!(second.contains("Answered: yes"), "{second}");
    assert!(!second.contains("\"actions\""), "{second}");
}

/// Saiful's thread reply is the answer, and the message loses its buttons.
#[test]
fn a_thread_reply_answers_the_question_on_the_hub_that_asked() {
    let p = pair("ls-reply");
    asked(&p, &["yes", "no"]);
    let waiter = waiting(&p.asker_dir.join("local.sock"), 10);
    thread::sleep(Duration::from_millis(200));
    p.asker_slack.event(reply_by(SAIFUL, "use hash"));
    let (response, took) = waiter.join().unwrap();
    assert_eq!(status_of(&response), 200, "{response}");
    assert_eq!(body_of(&response), "use hash");
    assert!(took < Duration::from_secs(2), "{took:?}");

    wait_until("mem answer", || !mem_log(&p.asker_dir).is_empty());
    assert_eq!(
        mem_log(&p.asker_dir),
        ["answer --by slack -- ABCD1234 use hash"]
    );
    wait_until("the message marked answered", || {
        !p.asker_slack.calls_to("chat.update").is_empty()
    });
    let update: Value = serde_json::from_str(&p.asker_slack.calls_to("chat.update")[0]).unwrap();
    assert_eq!(update["ts"], "1700000000.000100");
    assert!(
        update["text"]
            .as_str()
            .unwrap()
            .ends_with("Answered: use hash")
    );
    assert!(!update.to_string().contains("\"actions\""), "{update}");
}

#[test]
fn a_thread_reply_on_another_machine_reaches_the_hub_that_asked() {
    let p = pair("ls-reply-fwd");
    asked(&p, &["yes", "no"]);
    let waiter = waiting(&p.asker_dir.join("local.sock"), 10);
    thread::sleep(Duration::from_millis(200));
    p.taker_slack.event(reply_by(SAIFUL, "use hash"));
    let (response, took) = waiter.join().unwrap();
    assert_eq!(status_of(&response), 200, "{response}");
    assert_eq!(body_of(&response), "use hash");
    assert!(took < Duration::from_secs(2), "{took:?}");

    wait_until("mem answer", || !mem_log(&p.asker_dir).is_empty());
    assert_eq!(
        mem_log(&p.asker_dir),
        ["answer --by slack -- ABCD1234 use hash"]
    );
    wait_until("the message marked answered", || {
        !p.taker_slack.calls_to("chat.update").is_empty()
    });
}

/// Only Saiful answers: never someone else, and never the bot's own post.
#[test]
fn a_reply_by_anyone_else_or_by_the_bot_answers_nothing() {
    let p = pair("ls-reply-refuse");
    asked(&p, &["yes", "no"]);
    p.asker_slack.event(reply_by("U0SOMEONE", "use hash"));
    let mut bot = reply_by(SAIFUL, "use hash");
    bot["bot_id"] = json!("B0BOT");
    p.asker_slack.event(bot);
    wait_until("both envelopes acked", || {
        p.asker_slack
            .acks
            .lock()
            .unwrap()
            .iter()
            .filter(|a| a.contains("env-2"))
            .count()
            == 2
    });
    let (response, _) = waiting(&p.asker_dir.join("local.sock"), 1).join().unwrap();
    assert_eq!(status_of(&response), 204, "{response}");
    assert!(p.asker_slack.calls_to("chat.update").is_empty());
    assert!(mem_log(&p.asker_dir).is_empty());
}

/// A photo with no words is evidence on the machine that asked, whichever
/// hub Slack handed it to.
#[test]
fn a_photo_alone_becomes_evidence_on_the_hub_that_asked() {
    let p = pair("ls-photo");
    asked(&p, &["yes", "no"]);
    let url = p.taker_slack.file("photo.png", PHOTO);
    p.taker_slack.event(with_photo(reply_by(SAIFUL, ""), &url));

    wait_until("mem evidence add", || !mem_log(&p.asker_dir).is_empty());
    let log = mem_log(&p.asker_dir);
    assert_eq!(log.len(), 1, "{log:?}");
    // Filed in the question's project: the hub runs from $HOME, where mem
    // could not tell which project is meant.
    assert!(
        log[0].starts_with("--project=walk evidence add --task #ABCD1234 "),
        "{log:?}"
    );
    assert!(
        log[0].ends_with(" --note photo from Slack for #ABCD1234"),
        "{log:?}"
    );
    assert_eq!(
        std::fs::read(p.asker_dir.join("evidence.bin")).unwrap(),
        PHOTO
    );
    let (response, _) = waiting(&p.asker_dir.join("local.sock"), 1).join().unwrap();
    assert_eq!(status_of(&response), 204, "{response}");
}

/// A photo that comes with words is the answer's screenshot, not evidence.
#[test]
fn a_photo_with_words_is_the_answers_screenshot() {
    let p = pair("ls-photo-text");
    asked(&p, &["yes", "no"]);
    let url = p.asker_slack.file("photo.png", PHOTO);
    p.asker_slack
        .event(with_photo(reply_by(SAIFUL, "like this"), &url));

    wait_until("mem answer", || !mem_log(&p.asker_dir).is_empty());
    let log = mem_log(&p.asker_dir);
    assert_eq!(log.len(), 1, "{log:?}");
    assert!(
        log[0].starts_with("answer --by slack --screenshot "),
        "{log:?}"
    );
    assert!(log[0].ends_with(" -- ABCD1234 like this"), "{log:?}");
    assert_eq!(std::fs::read(p.asker_dir.join("shot.bin")).unwrap(), PHOTO);
    assert!(!p.asker_dir.join("evidence.bin").exists());
}

/// A question with no options has no buttons, so its message names it in
/// metadata, and a reply under it still finds it.
#[test]
fn a_question_without_options_carries_metadata_and_is_answered_in_its_thread() {
    let p = pair("ls-no-options");
    let posted: Value = serde_json::from_str(&asked(&p, &[])).unwrap();
    let metadata = &posted["metadata"];
    assert_eq!(metadata["event_type"], "workflow_question");
    assert_eq!(metadata["event_payload"]["q"], "ABCD1234");
    assert!(
        metadata["event_payload"]["m"]
            .as_str()
            .unwrap()
            .starts_with("127.0.0.1:"),
        "{metadata}"
    );

    let waiter = waiting(&p.asker_dir.join("local.sock"), 10);
    thread::sleep(Duration::from_millis(200));
    p.taker_slack.event(reply_by(SAIFUL, "call it hub"));
    let (response, _) = waiter.join().unwrap();
    assert_eq!(status_of(&response), 200, "{response}");
    assert_eq!(body_of(&response), "call it hub");
}

#[test]
fn an_option_is_its_buttons_text_exactly() {
    let p = pair("ls-labels");
    let posted: Value = serde_json::from_str(&asked(&p, &["a b", "yes, all"])).unwrap();
    let buttons = posted["blocks"][1]["elements"].as_array().unwrap();
    let labels: Vec<&str> = buttons
        .iter()
        .map(|b| b["text"]["text"].as_str().unwrap())
        .collect();
    assert_eq!(labels, ["a b", "yes, all"]);
}

/// A reply made while no hub was connected is read when the asking hub
/// starts again.
#[test]
fn a_reply_made_while_the_hub_was_away_answers_when_it_starts() {
    let dir = TempDir::new("ls-catch-up");
    let log = dir.join("stderr.log");
    let first = fake();
    let hub = spawn_local(&dir, "cloudbox", &first, &log);
    let port = hub.port;
    ask(&dir.join("local.sock"), "human");
    wait_until("the question posted", || {
        !first.calls_to("chat.postMessage").is_empty()
    });
    drop(hub);

    first
        .dm
        .lock()
        .unwrap()
        .messages
        .push(reply_by(SAIFUL, "use hash"));
    let second = fake_on(Arc::clone(&first.dm));
    let _hub = spawn_local_on(&dir, "cloudbox", &second, &log, port);
    let (response, _) = waiting(&dir.join("local.sock"), 10).join().unwrap();
    assert_eq!(status_of(&response), 200, "{response}");
    assert_eq!(body_of(&response), "use hash");
    wait_until("mem answer", || !mem_log(&dir).is_empty());
    assert_eq!(mem_log(&dir), ["answer --by slack -- ABCD1234 use hash"]);
}

/// Slack escapes `&`, `<` and `>` in a message's text; the answer is what
/// Saiful typed.
#[test]
fn a_thread_reply_is_recorded_as_typed_not_as_slack_escapes_it() {
    let p = pair("ls-reply-escaped");
    asked(&p, &["yes", "no"]);
    let waiter = waiting(&p.asker_dir.join("local.sock"), 10);
    thread::sleep(Duration::from_millis(200));
    p.asker_slack
        .event(reply_by(SAIFUL, "use &lt;hash&gt; &amp; keep &amp;lt;"));
    let (response, _) = waiter.join().unwrap();
    assert_eq!(body_of(&response), "use <hash> & keep &lt;");
}

/// A photo posted alone was filed when it came; reading the thread again at
/// a start must not file it twice.
#[test]
fn a_start_reads_only_the_answer_from_a_thread_not_its_photos_again() {
    let dir = TempDir::new("ls-catch-up-photo");
    let log = dir.join("stderr.log");
    let first = fake();
    let hub = spawn_local(&dir, "cloudbox", &first, &log);
    let port = hub.port;
    ask(&dir.join("local.sock"), "human");
    wait_until("the question posted", || {
        !first.calls_to("chat.postMessage").is_empty()
    });
    drop(hub);

    let second = fake_on(Arc::clone(&first.dm));
    let url = second.file("photo.png", PHOTO);
    let mut photo = with_photo(reply_by(SAIFUL, ""), &url);
    photo["ts"] = json!("1700000000.500000");
    first.dm.lock().unwrap().messages.push(photo);
    let _hub = spawn_local_on(&dir, "cloudbox", &second, &log, port);
    thread::sleep(Duration::from_millis(800));
    assert!(mem_log(&dir).is_empty(), "{:?}", mem_log(&dir));
}

/// A real photo is bigger than an ordinary request: through the hub whose
/// Slack connection got it, it still reaches the asking hub whole.
#[test]
fn a_photo_bigger_than_a_request_reaches_the_hub_that_asked() {
    let p = pair("ls-photo-big");
    asked(&p, &["yes", "no"]);
    let big: Vec<u8> = (0..300_000u32).map(|i| (i * 31 % 253) as u8).collect();
    let url = p.taker_slack.file("big.png", &big);
    p.taker_slack.event(with_photo(reply_by(SAIFUL, ""), &url));
    wait_until("mem evidence add", || !mem_log(&p.asker_dir).is_empty());
    assert_eq!(
        std::fs::read(p.asker_dir.join("evidence.bin")).unwrap(),
        big
    );
}

/// A hub that runs walk01, stalled: its mem lists walk01 with this
/// machine (`mini`) as runner, its amx lists the session stopped, and its
/// systemd-run records each call to `ran`. It posts the stall itself.
fn spawn_runner(dir: &TempDir, slack: &Fake, log: &Path) -> Hub {
    spawn_runner_with(
        dir,
        slack,
        log,
        "walk01-w2",
        r#"[{"id":"walk01-w2","state":"stopped","ended":9,"created":5}]"#,
    )
}

/// `spawn_runner` with the runner's amx id and what amx lists.
fn spawn_runner_with(dir: &TempDir, slack: &Fake, log: &Path, runner: &str, amx: &str) -> Hub {
    let bin = dir.join("bin");
    std::fs::write(
        dir.join("projects.json"),
        json!({"projects": [{
            "name": "walk01", "checkouts": ["/x"], "roadmap_status": "approved",
            "milestone": "w2", "milestones_done": 1, "milestones_total": 3,
            "runner": {"machine": "mini", "id": runner, "milestone": null,
                       "since": "2026-10-16T00:00:00Z"}}]})
        .to_string(),
    )
    .unwrap();
    std::fs::write(dir.join("amx.json"), amx).unwrap();
    std::fs::write(dir.join("handoff.json"), r#"{"body":"Next: w2."}"#).unwrap();
    common::fixture_mem(
        &bin,
        &format!(
            "case \"$*\" in\n\
             *questions*) echo '{{\"questions\":[]}}' ;;\n\
             'projects --json') cat '{d}/projects.json' ;;\n\
             handoff*) cat '{d}/handoff.json' ;;\n\
             esac",
            d = dir.path().display()
        ),
    );
    common::fixture_bin(
        &bin,
        "amx",
        &format!("cat '{}/amx.json'", dir.path().display()),
    );
    common::fixture_bin(
        &bin,
        "systemd-run",
        &format!("echo \"$*\" >> '{}/ran'", dir.path().display()),
    );
    let home = dir.join("home");
    std::fs::create_dir_all(home.join("config/qshell")).unwrap();
    std::fs::write(home.join("config/qshell/machine"), "mac-mini").unwrap();
    let socket = dir.join("local.sock");
    Hub::spawn_with_stderr(
        &home,
        &[&bin],
        &["local", "--port", "0"],
        &[
            ("HUB_PEER_TOKEN", TOKEN),
            ("HUB_LOCAL_SOCKET", socket.to_str().unwrap()),
            ("HUB_HOST", "127.0.0.1"),
            ("WORKFLOW_MACHINE", "mini"),
            ("HUB_STALL_CHECK_MS", "100"),
            ("HUB_STALL_AFTER_MS", "200"),
            ("SLACK_BOT_TOKEN", BOT),
            ("SLACK_APP_TOKEN", APP),
            ("SLACK_USER_ID", SAIFUL),
            ("SLACK_API_URL", &slack.api),
        ],
        std::fs::File::create(log).unwrap(),
    )
}

fn ran(dir: &TempDir) -> Vec<String> {
    std::fs::read_to_string(dir.join("ran"))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

/// The Resume button's value on the stall the runner posted.
fn resume_value(slack: &Fake) -> String {
    wait_until("the stall posted", || {
        slack
            .calls_to("chat.postMessage")
            .iter()
            .any(|p| p.contains("stalled on mini"))
    });
    let posted = slack
        .calls_to("chat.postMessage")
        .into_iter()
        .find(|p| p.contains("stalled on mini"))
        .unwrap();
    let message: Value = serde_json::from_str(&posted).unwrap();
    message["blocks"][1]["elements"][0]["value"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn a_resume_tap_on_another_machine_starts_the_run_on_its_runner_once() {
    let runner_dir = TempDir::new("ls-resume-runner");
    let taker_dir = TempDir::new("ls-resume-taker");
    let runner_slack = fake();
    let taker_slack = fake_on(Arc::clone(&runner_slack.dm));
    let _runner = spawn_runner(&runner_dir, &runner_slack, &runner_dir.join("err.log"));
    let _taker = spawn_local(&taker_dir, "nuc", &taker_slack, &taker_dir.join("err.log"));
    let value = resume_value(&runner_slack);
    let carried: Value = serde_json::from_str(&value).unwrap();
    assert_eq!(carried["k"], "resume");
    assert_eq!(carried["m"], "mini");
    assert_eq!(carried["p"], "walk01");
    assert!(
        carried["h"].as_str().unwrap().starts_with("127.0.0.1:"),
        "the button names the runner's hub: {value}"
    );

    // Slack hands the tap to the nuc's connection.
    let at = Instant::now();
    taker_slack.tap(SAIFUL, &value);
    wait_until("the start", || !ran(&runner_dir).is_empty());
    assert!(at.elapsed() < Duration::from_secs(5), "{:?}", at.elapsed());
    assert_eq!(
        ran(&runner_dir),
        ["--user --scope --quiet workflow go walk01"]
    );
    wait_until("the stall message updated", || {
        taker_slack
            .calls_to("chat.update")
            .iter()
            .any(|u| u.contains("resumed on mini"))
    });
    let update: Value = serde_json::from_str(&taker_slack.calls_to("chat.update")[0]).unwrap();
    assert!(update["blocks"][1].is_null(), "no button is left: {update}");

    // A second tap on the same button starts nothing more.
    taker_slack.tap(SAIFUL, &value);
    thread::sleep(Duration::from_millis(800));
    assert_eq!(ran(&runner_dir).len(), 1, "{:?}", ran(&runner_dir));
}

#[test]
fn a_resume_tap_on_the_runners_own_connection_starts_it_there() {
    let dir = TempDir::new("ls-resume-own");
    let slack = fake();
    let _runner = spawn_runner(&dir, &slack, &dir.join("err.log"));
    let value = resume_value(&slack);
    slack.tap(SAIFUL, &value);
    wait_until("the start", || !ran(&dir).is_empty());
    assert_eq!(ran(&dir), ["--user --scope --quiet workflow go walk01"]);
}

#[test]
fn a_resume_tap_by_anyone_but_saiful_starts_nothing() {
    let dir = TempDir::new("ls-resume-refuse");
    let slack = fake();
    let _runner = spawn_runner(&dir, &slack, &dir.join("err.log"));
    let value = resume_value(&slack);
    slack.tap("U0SOMEONE", &value);
    thread::sleep(Duration::from_millis(800));
    assert!(ran(&dir).is_empty(), "{:?}", ran(&dir));
}

#[test]
fn a_forged_resume_on_the_peer_port_needs_the_token_and_a_stalled_run_here() {
    let dir = TempDir::new("ls-resume-forge");
    let slack = fake();
    let runner = spawn_runner(&dir, &slack, &dir.join("err.log"));
    let body = r#"{"project":"walk01"}"#;
    let (status, _) = hub::local::request(
        &runner.origin()[7..],
        "POST",
        "/resume",
        "wrong",
        body.as_bytes(),
    )
    .unwrap();
    assert_eq!(status, 401);
    let (status, _) = hub::local::request(
        &runner.origin()[7..],
        "POST",
        "/resume",
        TOKEN,
        br#"{"project":"walk09"}"#,
    )
    .unwrap();
    assert_eq!(status, 404, "not a run of this machine");
    assert!(ran(&dir).is_empty(), "{:?}", ran(&dir));
}

/// The usual stall after a landing: the runner is the last milestone's
/// orchestrator, idle in amx, and no successor started. The bell and the
/// tap read it alike, so the tap starts the next one.
#[test]
fn a_resume_tap_starts_a_stall_whose_runner_finished_an_earlier_milestone() {
    let dir = TempDir::new("ls-resume-landed");
    let slack = fake();
    let _runner = spawn_runner_with(
        &dir,
        &slack,
        &dir.join("err.log"),
        "walk01-w1",
        r#"[{"id":"walk01-w1","state":"idle","ended":0,"created":5}]"#,
    );
    let value = resume_value(&slack);
    slack.tap(SAIFUL, &value);
    wait_until("the start", || !ran(&dir).is_empty());
    assert_eq!(ran(&dir), ["--user --scope --quiet workflow go walk01"]);
}

/// A stall's old button tapped after the run went on and parked on a
/// question starts nothing beside the parked session.
#[test]
fn a_resume_tap_on_a_run_that_has_parked_since_starts_nothing() {
    let dir = TempDir::new("ls-resume-parked");
    let slack = fake();
    let _runner = spawn_runner(&dir, &slack, &dir.join("err.log"));
    let value = resume_value(&slack);
    std::fs::write(
        dir.join("handoff.json"),
        r#"{"body":"parked: waiting on #Q1"}"#,
    )
    .unwrap();
    slack.tap(SAIFUL, &value);
    wait_until("the stall message updated", || {
        slack
            .calls_to("chat.update")
            .iter()
            .any(|u| u.contains("walk01"))
    });
    assert!(ran(&dir).is_empty(), "{:?}", ran(&dir));
}
