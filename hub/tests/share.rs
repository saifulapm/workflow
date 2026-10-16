//! `hub local` holds this machine's sharing socket: a fake store on
//! 127.0.0.1 and a fixture `mem` stand in for Cloudflare and the replica.

mod common;

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{Hub, TempDir, fixture_mem, status_of, wait_for};
use tungstenite::Message;
use tungstenite::handshake::server::{Request, Response};

const PEER_TOKEN: &str = "peer-token-for-tests-0123456789abcdef";
const STORE_TOKEN: &str = "store-token-for-tests-fedcba9876543210";
const HELLO: &str = r#"{"t":"hello","machine":"testbox","since":41}"#;
const WRITE_A: &str = r#"{"t":"write","id":"01J00000000000000000000AAA","path":"projects/P/wiki/a.md","base":null,"body":"a"}"#;
const LIMIT: &str = r#"{"t":"limit"}"#;
const NOTICE: &str = "Cloudflare limit reached, resets 06:00";
const WRITE_B: &str = r#"{"t":"write","id":"01J00000000000000000000BBB","path":"projects/P/wiki/b.md","base":null,"body":"b"}"#;
const ACCESS_ID: &str = "access-id-for-tests.access";
const ACCESS_SECRET: &str = "access-secret-for-tests-0123456789";
const ACCESS: [(&str, &str); 2] = [
    ("HUB_STORE_ACCESS_ID", ACCESS_ID),
    ("HUB_STORE_ACCESS_SECRET", ACCESS_SECRET),
];
const SHOT: &str = "projects/P/evidence/t1/a b.png";
/// SHOT as the hub puts it in a URL.
const SHOT_URL: &str = "/store/files/projects/P/evidence/t1/a%20b.png";
const STORED: &str = r#"{"t":"stored","path":"projects/P/evidence/t1/a b.png"}"#;
/// Not UTF-8, so a text round trip would show.
const SHOT_BYTES: &[u8] = &[0x89, b'P', b'N', b'G', 0xff, 0x00, 0xfe, 0x80, b'\n'];

/// What the fake store saw, connection by connection.
#[derive(Default)]
struct Seen {
    /// The Authorization header of each connection, in order.
    auth: Vec<Option<String>>,
    /// The CF-Access-Client-Id and -Secret headers of each connection.
    access: Vec<(Option<String>, Option<String>)>,
    /// Each text frame, with the index of the connection it came on.
    texts: Vec<(usize, String)>,
    /// Each frame that is not text, by kind ("ping", "binary", ...), with
    /// the index of the connection it came on.
    others: Vec<(usize, &'static str)>,
}

/// One plain HTTP request to the fake's files routes.
#[derive(Clone, Debug)]
struct Asked {
    method: String,
    path: String,
    /// Names lowercased.
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Asked {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

/// The fake's files store: what it was asked and how it answers.
struct Files {
    asked: Vec<Asked>,
    /// What a GET answers 200 with, by request path; any other path is 404.
    stored: HashMap<String, Vec<u8>>,
    put_status: u16,
    /// How long a PUT is held after its body arrived.
    put_delay: Duration,
}

enum Order {
    Send(String),
    Drop,
}

/// A store that takes one connection at a time, records what arrives and
/// sends or drops on order.
/// The hub reaches the files on the socket's host and port, so the one
/// listener answers both.
struct FakeStore {
    url: String,
    seen: Arc<Mutex<Seen>>,
    files: Arc<Mutex<Files>>,
    orders: Sender<Order>,
}

impl FakeStore {
    fn start() -> FakeStore {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("ws://{}/store/share", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Seen::default()));
        let files = Arc::new(Mutex::new(Files {
            asked: Vec::new(),
            stored: HashMap::new(),
            put_status: 204,
            put_delay: Duration::ZERO,
        }));
        let (orders, rx) = channel();
        let (sockets, streams) = channel();
        let recorded = Arc::clone(&seen);
        std::thread::spawn(move || serve(streams, recorded, rx));
        let answering = Arc::clone(&files);
        std::thread::spawn(move || dispatch(listener, sockets, answering));
        FakeStore {
            url,
            seen,
            files,
            orders,
        }
    }

    fn asked(&self) -> Vec<Asked> {
        self.files.lock().unwrap().asked.clone()
    }

    fn puts(&self) -> Vec<Asked> {
        self.asked()
            .into_iter()
            .filter(|a| a.method == "PUT")
            .collect()
    }

    fn send(&self, text: &str) {
        self.orders.send(Order::Send(text.to_string())).unwrap();
    }

    fn drop_connection(&self) {
        self.orders.send(Order::Drop).unwrap();
    }

    fn texts_on(&self, conn: usize) -> Vec<String> {
        let seen = self.seen.lock().unwrap();
        seen.texts
            .iter()
            .filter(|(c, _)| *c == conn)
            .map(|(_, t)| t.clone())
            .collect()
    }
}

/// Hands each WebSocket upgrade to `serve` and answers every other request
/// on a thread of its own, so a held PUT never holds up the socket.
fn dispatch(listener: TcpListener, sockets: Sender<TcpStream>, files: Arc<Mutex<Files>>) {
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let mut start = [0u8; 16];
        let upgrade = loop {
            match stream.peek(&mut start) {
                Ok(n) if n < start.len() && n > 0 => std::thread::sleep(Duration::from_millis(5)),
                Ok(_) => break start.starts_with(b"GET /store/share"),
                Err(_) => break false,
            }
        };
        if upgrade {
            let _ = sockets.send(stream);
        } else {
            let files = Arc::clone(&files);
            std::thread::spawn(move || answer_files(stream, &files));
        }
    }
}

fn answer_files(mut stream: TcpStream, files: &Mutex<Files>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let mut words = line.split(' ');
    let method = words.next().unwrap_or_default().to_string();
    let path = words.next().unwrap_or_default().to_string();
    let mut headers = Vec::new();
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).unwrap();
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            headers.push((name.to_ascii_lowercase(), value.trim().to_string()));
        }
    }
    let length = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .map_or(0, |(_, v)| v.parse().unwrap());
    let mut body = vec![0; length];
    reader.read_exact(&mut body).unwrap();
    let (status, reply, delay) = {
        let mut files = files.lock().unwrap();
        let answer = match (method.as_str(), files.stored.get(&path)) {
            ("PUT", _) => (files.put_status, Vec::new(), files.put_delay),
            ("GET", Some(bytes)) => (200, bytes.clone(), Duration::ZERO),
            _ => (404, Vec::new(), Duration::ZERO),
        };
        files.asked.push(Asked {
            method,
            path,
            headers,
            body,
        });
        answer
    };
    std::thread::sleep(delay);
    let _ = write!(
        stream,
        "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        reply.len()
    );
    let _ = stream.write_all(&reply);
}

// tungstenite fixes the header callback's error type, a whole Response.
#[allow(clippy::result_large_err)]
fn serve(streams: Receiver<TcpStream>, seen: Arc<Mutex<Seen>>, orders: Receiver<Order>) {
    for (conn, stream) in streams.iter().enumerate() {
        let auth = Arc::clone(&seen);
        let socket =
            tungstenite::accept_hdr(stream, move |request: &Request, response: Response| {
                let header = |name| {
                    request
                        .headers()
                        .get(name)
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_string)
                };
                let mut seen = auth.lock().unwrap();
                seen.auth.push(header("authorization"));
                seen.access.push((
                    header("cf-access-client-id"),
                    header("cf-access-client-secret"),
                ));
                Ok(response)
            });
        let Ok(mut socket) = socket else { continue };
        socket
            .get_mut()
            .set_read_timeout(Some(Duration::from_millis(20)))
            .unwrap();
        'conn: loop {
            while let Ok(order) = orders.try_recv() {
                match order {
                    Order::Send(text) => socket.send(Message::text(text)).unwrap(),
                    Order::Drop => break 'conn,
                }
            }
            match socket.read() {
                Ok(Message::Text(text)) => {
                    seen.lock().unwrap().texts.push((conn, text.to_string()));
                }
                Ok(other) => {
                    let kind = match other {
                        Message::Ping(_) => "ping",
                        Message::Pong(_) => "pong",
                        Message::Binary(_) => "binary",
                        Message::Close(_) => "close",
                        _ => "frame",
                    };
                    seen.lock().unwrap().others.push((conn, kind));
                }
                Err(tungstenite::Error::Io(e))
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(_) => break,
            }
        }
    }
}

/// A `mem` whose `share hello` prints `HELLO`, whose `share pending` prints
/// the file `pending`, and whose `share apply` appends stdin to `applied`.
fn share_mem(dir: &TempDir) -> PathBuf {
    let bin = dir.join("bin");
    fixture_mem(
        &bin,
        &format!(
            "case \"$1 $2\" in\n\
             'share hello') echo '{HELLO}' ;;\n\
             'share pending') cat '{pending}' 2>/dev/null || true ;;\n\
             'share apply') cat >> '{applied}' ;;\n\
             esac",
            pending = dir.join("pending").display(),
            applied = dir.join("applied").display(),
        ),
    );
    bin
}

fn spawn_local(dir: &TempDir, bin: &Path, store: Option<&FakeStore>) -> Hub {
    spawn_local_env(dir, bin, store, &[])
}

/// `spawn_local`, with `extra` in the hub's environment too.
fn spawn_local_env(
    dir: &TempDir,
    bin: &Path,
    store: Option<&FakeStore>,
    extra: &[(&str, &str)],
) -> Hub {
    let home = dir.join("home");
    std::fs::create_dir_all(home.join("config/qshell")).unwrap();
    std::fs::write(home.join("config/qshell/machine"), "testbox\n").unwrap();
    let socket = dir.join("local.sock");
    let mut env = vec![
        ("HUB_PEER_TOKEN", PEER_TOKEN),
        ("HUB_LOCAL_SOCKET", socket.to_str().unwrap()),
    ];
    if let Some(store) = store {
        env.push(("HUB_STORE_URL", store.url.as_str()));
        env.push(("HUB_STORE_TOKEN", STORE_TOKEN));
    }
    env.extend_from_slice(extra);
    Hub::spawn_env(&home, &[bin], &["local", "--port", "0"], &env)
}

fn on_socket(socket: &Path, request: &str) -> String {
    let mut stream = UnixStream::connect(socket).unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

/// The status and body of a request on mem's socket, the body as bytes.
fn on_socket_bytes(socket: &Path, request: &str) -> (u16, Vec<u8>) {
    let mut stream = UnixStream::connect(socket).unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).unwrap();
    let end = response
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("a response head");
    let head = String::from_utf8_lossy(&response[..end]).into_owned();
    (status_of(&head), response[end + 4..].to_vec())
}

fn get_file(dir: &TempDir, query: &str) -> (u16, Vec<u8>) {
    on_socket_bytes(
        &dir.join("local.sock"),
        &format!("GET /share/file?path={query} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"),
    )
}

/// A pending line for SHOT, its bytes in `dir`.
fn pending_shot(dir: &TempDir) -> String {
    let local = dir.join("a b.png");
    std::fs::write(&local, SHOT_BYTES).unwrap();
    serde_json::json!({"t": "file", "path": SHOT, "local": local}).to_string()
}

fn applied(dir: &TempDir) -> String {
    std::fs::read_to_string(dir.join("applied")).unwrap_or_default()
}

fn kick(dir: &TempDir) -> String {
    on_socket(
        &dir.join("local.sock"),
        "POST /share/kick HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\n\r\n",
    )
}

#[test]
fn the_store_gets_the_bearer_then_the_hello_mem_printed() {
    let dir = TempDir::new("share-hello");
    let bin = share_mem(&dir);
    let store = FakeStore::start();
    let _hub = spawn_local(&dir, &bin, Some(&store));
    wait_for("a hello", Duration::from_secs(5), || {
        !store.texts_on(0).is_empty()
    });
    assert_eq!(
        store.seen.lock().unwrap().auth[0].as_deref(),
        Some(format!("Bearer {STORE_TOKEN}").as_str())
    );
    assert_eq!(store.texts_on(0)[0], HELLO);
}

#[test]
fn pending_lines_follow_the_hello_and_a_kick_sends_a_new_one_once() {
    let dir = TempDir::new("share-pending");
    let bin = share_mem(&dir);
    std::fs::write(dir.join("pending"), format!("{WRITE_A}\n")).unwrap();
    let store = FakeStore::start();
    let _hub = spawn_local(&dir, &bin, Some(&store));
    wait_for("the pending write", Duration::from_secs(5), || {
        store.texts_on(0).len() >= 2
    });
    assert_eq!(store.texts_on(0), [HELLO, WRITE_A]);

    std::fs::write(dir.join("pending"), format!("{WRITE_A}\n{WRITE_B}\n")).unwrap();
    let kicked = kick(&dir);
    assert_eq!(status_of(&kicked), 204, "{kicked}");
    let at = Instant::now();
    wait_for("the kicked write", Duration::from_secs(1), || {
        store.texts_on(0).len() >= 3
    });
    assert!(at.elapsed() < Duration::from_secs(1));
    // A second kick with nothing new sends nothing more on this connection.
    assert_eq!(status_of(&kick(&dir)), 204);
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(store.texts_on(0), [HELLO, WRITE_A, WRITE_B]);
}

#[test]
fn every_frame_from_the_store_reaches_mem_share_apply_in_order() {
    let dir = TempDir::new("share-apply");
    let bin = share_mem(&dir);
    let store = FakeStore::start();
    let _hub = spawn_local(&dir, &bin, Some(&store));
    wait_for("a hello", Duration::from_secs(5), || {
        !store.texts_on(0).is_empty()
    });
    let frames = [
        r#"{"t":"rec","seq":42,"id":"01J00000000000000000000CCC","path":"projects/P/wiki/c.md","body":"c","machine":"nuc"}"#,
        r#"{"t":"ready","seq":42}"#,
        r#"{"t":"ack","id":"01J00000000000000000000AAA","seq":43}"#,
    ];
    for frame in frames {
        store.send(frame);
    }
    let applied = dir.join("applied");
    let want = format!("{}\n", frames.join("\n"));
    wait_for("the frames applied", Duration::from_secs(1), || {
        std::fs::read_to_string(&applied).unwrap_or_default() == want
    });
}

#[test]
fn a_dropped_socket_comes_back_with_a_hello_and_every_pending_line() {
    let dir = TempDir::new("share-reconnect");
    let bin = share_mem(&dir);
    std::fs::write(dir.join("pending"), format!("{WRITE_A}\n{WRITE_B}\n")).unwrap();
    let store = FakeStore::start();
    let _hub = spawn_local(&dir, &bin, Some(&store));
    wait_for("the first connection", Duration::from_secs(5), || {
        store.texts_on(0).len() >= 3
    });
    store.drop_connection();
    wait_for("the second connection", Duration::from_secs(8), || {
        store.texts_on(1).len() >= 3
    });
    assert_eq!(store.texts_on(1), [HELLO, WRITE_A, WRITE_B]);
    assert_eq!(
        store.seen.lock().unwrap().auth[1].as_deref(),
        Some(format!("Bearer {STORE_TOKEN}").as_str())
    );
}

/// mem raises its `since` only for the recs it committed, so a failed apply
/// ends the connection: the next hello asks the store for the rest.
#[test]
fn a_failed_apply_reconnects_and_later_frames_still_reach_mem() {
    let dir = TempDir::new("share-apply-fails");
    let bin = dir.join("bin");
    fixture_mem(
        &bin,
        &format!(
            "case \"$1 $2\" in\n\
             'share hello') echo '{HELLO}' ;;\n\
             'share pending') ;;\n\
             'share apply') if [ -e '{failed}' ]; then cat >> '{applied}'; \
             else touch '{failed}'; exit 1; fi ;;\n\
             esac",
            failed = dir.join("failed").display(),
            applied = dir.join("applied").display(),
        ),
    );
    let store = FakeStore::start();
    let _hub = spawn_local(&dir, &bin, Some(&store));
    wait_for("a hello", Duration::from_secs(5), || {
        !store.texts_on(0).is_empty()
    });
    let lost = r#"{"t":"rec","seq":42,"id":"01J00000000000000000000CCC","path":"projects/P/wiki/c.md","body":"c","machine":"nuc"}"#;
    store.send(lost);
    wait_for("a second hello", Duration::from_secs(5), || {
        !store.texts_on(1).is_empty()
    });
    assert_eq!(store.texts_on(1), [HELLO]);

    let replayed = r#"{"t":"ready","seq":42}"#;
    store.send(replayed);
    wait_for(
        "the frame after the reconnect",
        Duration::from_secs(2),
        || applied(&dir) == format!("{replayed}\n"),
    );
}

/// The runtime answers a protocol ping without waking the store, so an idle
/// hub costs the store no requests.
#[test]
fn an_idle_hub_sends_the_store_only_pings() {
    let dir = TempDir::new("share-idle");
    let bin = share_mem(&dir);
    let store = FakeStore::start();
    let _hub = spawn_local_env(&dir, &bin, Some(&store), &[("HUB_SHARE_PING_MS", "200")]);
    wait_for("a hello", Duration::from_secs(5), || {
        !store.texts_on(0).is_empty()
    });
    let before = store.seen.lock().unwrap().others.len();
    std::thread::sleep(Duration::from_millis(1500));
    let seen = store.seen.lock().unwrap();
    let watched = &seen.others[before..];
    assert!(
        watched
            .iter()
            .all(|&(conn, kind)| (conn, kind) == (0, "ping")),
        "{watched:?}"
    );
    assert!(watched.len() >= 3, "{watched:?}");
    assert_eq!(seen.texts, [(0, HELLO.to_string())]);
}

#[test]
fn a_kick_is_mems_alone_and_answers_even_with_sharing_off() {
    let dir = TempDir::new("share-kick");
    let bin = share_mem(&dir);
    let hub = spawn_local(&dir, &bin, None);
    let peer = hub.raw(&format!(
        "POST /share/kick HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {PEER_TOKEN}\r\n\
         Content-Length: 0\r\n\r\n"
    ));
    assert_eq!(status_of(&peer), 404, "{peer}");
    let mem = kick(&dir);
    assert_eq!(status_of(&mem), 204, "{mem}");
}

/// Sharing needs both variables. With only one of them set, the hub answers
/// on mem's socket and the store at `HUB_STORE_URL`, when there is one,
/// hears nothing.
#[test]
fn without_both_store_variables_nothing_connects_and_the_hub_still_serves() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("ws://{}/store/share", listener.local_addr().unwrap());
    for (name, value) in [("HUB_STORE_TOKEN", STORE_TOKEN), ("HUB_STORE_URL", &url)] {
        let dir = TempDir::new("share-off");
        let bin = share_mem(&dir);
        std::fs::write(dir.join("pending"), format!("{WRITE_A}\n")).unwrap();
        let home = dir.join("home");
        std::fs::create_dir_all(home.join("config/qshell")).unwrap();
        std::fs::write(home.join("config/qshell/machine"), "testbox\n").unwrap();
        let socket = dir.join("local.sock");
        let _hub = Hub::spawn_env(
            &home,
            &[&bin],
            &["local", "--port", "0"],
            &[
                ("HUB_PEER_TOKEN", PEER_TOKEN),
                ("HUB_LOCAL_SOCKET", socket.to_str().unwrap()),
                (name, value),
            ],
        );
        assert_eq!(status_of(&kick(&dir)), 204);
        std::thread::sleep(Duration::from_millis(1500));
        assert!(
            matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock),
            "the hub connected with only {name} set"
        );
        let ping = on_socket(&socket, "GET /ping HTTP/1.1\r\nHost: x\r\n\r\n");
        assert_eq!(status_of(&ping), 200, "{ping}");
    }
}

/// A Slack Web API that records the body of each chat.postMessage.
fn fake_slack() -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let api = format!("http://{}/api", listener.local_addr().unwrap());
    let posts = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&posts);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            answer_slack(stream, &recorded);
        }
    });
    (api, posts)
}

fn answer_slack(mut stream: TcpStream, posts: &Mutex<Vec<String>>) {
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
    let reply = if path == "/api/chat.postMessage" {
        posts.lock().unwrap().push(String::from_utf8(body).unwrap());
        r#"{"ok":true,"channel":"D0DM","ts":"1700000000.000100"}"#
    } else {
        r#"{"ok":false,"error":"unknown_method"}"#
    };
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
        reply.len()
    );
}

fn spawn_limited(
    dir: &TempDir,
    bin: &Path,
    store: &FakeStore,
    slack: Option<&str>,
    now: &str,
) -> Hub {
    let home = dir.join("home");
    std::fs::create_dir_all(home.join("config/qshell")).unwrap();
    std::fs::write(home.join("config/qshell/machine"), "testbox\n").unwrap();
    let socket = dir.join("local.sock");
    let clock = dir.join("now");
    std::fs::write(&clock, now).unwrap();
    let mut env = vec![
        ("HUB_PEER_TOKEN", PEER_TOKEN),
        ("HUB_LOCAL_SOCKET", socket.to_str().unwrap()),
        ("HUB_STORE_URL", store.url.as_str()),
        ("HUB_STORE_TOKEN", STORE_TOKEN),
        ("HUB_SHARE_NOW_FILE", clock.to_str().unwrap()),
    ];
    if let Some(api) = slack {
        env.extend([
            ("SLACK_BOT_TOKEN", "xoxb-fake"),
            ("SLACK_APP_TOKEN", "xapp-fake"),
            ("SLACK_USER_ID", "U0SAIFUL"),
            ("SLACK_API_URL", api),
        ]);
    }
    Hub::spawn_env(&home, &[bin], &["local", "--port", "0"], &env)
}

/// The store has refused the first write; returns once the hub has heard it.
fn hit_the_limit(dir: &TempDir, store: &FakeStore) {
    std::fs::write(dir.join("pending"), format!("{WRITE_A}\n")).unwrap();
    wait_for("the first write", Duration::from_secs(5), || {
        store.texts_on(0).len() >= 2
    });
    store.send(LIMIT);
    let applied = dir.join("applied");
    wait_for("the limit applied", Duration::from_secs(2), || {
        std::fs::read_to_string(&applied)
            .unwrap_or_default()
            .contains(LIMIT)
    });
}

#[test]
fn the_next_reset_is_the_next_midnight_utc() {
    let at = |s: &str| s.parse::<jiff::Timestamp>().unwrap();
    assert_eq!(
        hub::share::next_reset(at("2026-10-16T13:00:00+06:00")),
        at("2026-10-17T00:00:00Z")
    );
    assert_eq!(
        hub::share::next_reset(at("2026-10-16T05:59:00+06:00")),
        at("2026-10-16T00:00:00Z")
    );
    assert_eq!(
        hub::share::next_reset(at("2026-10-16T00:00:00Z")),
        at("2026-10-17T00:00:00Z")
    );
}

#[test]
fn after_a_limit_writes_are_held_but_the_stores_frames_still_apply() {
    let dir = TempDir::new("share-limit");
    let bin = share_mem(&dir);
    let store = FakeStore::start();
    let _hub = spawn_limited(&dir, &bin, &store, None, "2026-10-16T13:00:00+06:00");
    hit_the_limit(&dir, &store);

    std::fs::write(dir.join("pending"), format!("{WRITE_A}\n{WRITE_B}\n")).unwrap();
    assert_eq!(status_of(&kick(&dir)), 204);
    std::thread::sleep(Duration::from_millis(1000));
    assert_eq!(store.texts_on(0), [HELLO, WRITE_A]);

    let rec = r#"{"t":"rec","seq":42,"id":"01J00000000000000000000CCC","path":"projects/P/wiki/c.md","body":"c","machine":"nuc"}"#;
    store.send(rec);
    let applied = dir.join("applied");
    wait_for("the rec applied", Duration::from_secs(2), || {
        std::fs::read_to_string(&applied)
            .unwrap_or_default()
            .contains(rec)
    });
}

#[test]
fn past_the_reset_the_held_writes_go_out_again() {
    let dir = TempDir::new("share-resume");
    let bin = share_mem(&dir);
    let store = FakeStore::start();
    let _hub = spawn_limited(&dir, &bin, &store, None, "2026-10-16T13:00:00+06:00");
    hit_the_limit(&dir, &store);
    std::fs::write(dir.join("pending"), format!("{WRITE_A}\n{WRITE_B}\n")).unwrap();
    assert_eq!(status_of(&kick(&dir)), 204);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(store.texts_on(0), [HELLO, WRITE_A]);

    // 06:00 Dhaka the next day is 2026-10-17T00:00:00Z.
    std::fs::write(dir.join("now"), "2026-10-17T00:00:01Z").unwrap();
    wait_for("the held writes", Duration::from_secs(2), || {
        store.texts_on(0).len() >= 4
    });
    assert_eq!(store.texts_on(0), [HELLO, WRITE_A, WRITE_A, WRITE_B]);
}

#[test]
fn the_first_limit_of_a_dhaka_day_posts_one_dm_line_and_a_second_posts_none() {
    let dir = TempDir::new("share-notice");
    let bin = share_mem(&dir);
    let store = FakeStore::start();
    let (api, posts) = fake_slack();
    let _hub = spawn_limited(&dir, &bin, &store, Some(&api), "2026-10-16T13:00:00+06:00");
    hit_the_limit(&dir, &store);
    wait_for("the DM", Duration::from_secs(3), || {
        !posts.lock().unwrap().is_empty()
    });
    let post: serde_json::Value = serde_json::from_str(&posts.lock().unwrap()[0]).unwrap();
    assert_eq!(post["channel"], "U0SAIFUL");
    assert_eq!(post["text"], NOTICE);

    store.send(LIMIT);
    std::thread::sleep(Duration::from_millis(1000));
    assert_eq!(posts.lock().unwrap().len(), 1);
}

#[test]
fn a_pending_file_is_put_to_the_files_store_and_never_sent_on_the_socket() {
    let dir = TempDir::new("share-file-put");
    let bin = share_mem(&dir);
    let file = pending_shot(&dir);
    std::fs::write(dir.join("pending"), format!("{WRITE_A}\n{file}\n")).unwrap();
    let store = FakeStore::start();
    let _hub = spawn_local_env(&dir, &bin, Some(&store), &ACCESS);
    wait_for("the PUT", Duration::from_secs(5), || {
        !store.puts().is_empty()
    });
    let put = &store.puts()[0];
    assert_eq!(put.path, SHOT_URL);
    assert_eq!(put.body, SHOT_BYTES);
    assert_eq!(
        put.header("authorization"),
        Some(format!("Bearer {STORE_TOKEN}").as_str())
    );
    assert_eq!(put.header("cf-access-client-id"), Some(ACCESS_ID));
    assert_eq!(put.header("cf-access-client-secret"), Some(ACCESS_SECRET));
    wait_for("the stored line", Duration::from_secs(2), || {
        applied(&dir).contains(STORED)
    });
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(store.texts_on(0), [HELLO, WRITE_A]);
}

#[test]
fn with_both_access_variables_the_handshake_carries_them() {
    let dir = TempDir::new("share-access");
    let bin = share_mem(&dir);
    let store = FakeStore::start();
    let _hub = spawn_local_env(&dir, &bin, Some(&store), &ACCESS);
    wait_for("a hello", Duration::from_secs(5), || {
        !store.texts_on(0).is_empty()
    });
    assert_eq!(
        store.seen.lock().unwrap().access[0],
        (Some(ACCESS_ID.to_string()), Some(ACCESS_SECRET.to_string()))
    );
}

#[test]
fn a_failed_upload_is_not_marked_stored_and_a_kick_tries_it_again() {
    let dir = TempDir::new("share-file-retry");
    let bin = share_mem(&dir);
    let file = pending_shot(&dir);
    std::fs::write(dir.join("pending"), format!("{file}\n")).unwrap();
    let store = FakeStore::start();
    store.files.lock().unwrap().put_status = 500;
    let _hub = spawn_local(&dir, &bin, Some(&store));
    wait_for("the first PUT", Duration::from_secs(5), || {
        !store.puts().is_empty()
    });
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(store.puts().len(), 1, "a failed PUT waits for a kick");
    assert!(!applied(&dir).contains("stored"), "{}", applied(&dir));

    store.files.lock().unwrap().put_status = 204;
    assert_eq!(status_of(&kick(&dir)), 204);
    wait_for("the stored line", Duration::from_secs(2), || {
        applied(&dir).contains(STORED)
    });
    assert_eq!(store.puts().len(), 2);
}

#[test]
fn a_slow_upload_does_not_hold_up_what_the_store_sends() {
    let dir = TempDir::new("share-file-slow");
    let bin = share_mem(&dir);
    let file = pending_shot(&dir);
    std::fs::write(dir.join("pending"), format!("{file}\n")).unwrap();
    let store = FakeStore::start();
    store.files.lock().unwrap().put_delay = Duration::from_secs(3);
    let _hub = spawn_local(&dir, &bin, Some(&store));
    wait_for("the held PUT", Duration::from_secs(5), || {
        !store.puts().is_empty()
    });
    let rec = r#"{"t":"rec","seq":42,"id":"01J00000000000000000000CCC","path":"projects/P/wiki/c.md","body":"c","machine":"nuc"}"#;
    store.send(rec);
    wait_for("the rec applied", Duration::from_secs(1), || {
        applied(&dir).contains(rec)
    });
    assert!(!applied(&dir).contains("stored"), "the PUT is still held");
}

#[test]
fn mems_socket_fetches_a_file_from_the_store_byte_for_byte() {
    let dir = TempDir::new("share-file-get");
    let bin = share_mem(&dir);
    let store = FakeStore::start();
    store
        .files
        .lock()
        .unwrap()
        .stored
        .insert(SHOT_URL.to_string(), SHOT_BYTES.to_vec());
    let hub = spawn_local_env(&dir, &bin, Some(&store), &ACCESS);

    assert_eq!(
        get_file(&dir, "projects/P/evidence/t1/a%20b.png"),
        (200, SHOT_BYTES.to_vec())
    );
    let asked = store.asked();
    assert_eq!(asked.len(), 1);
    assert_eq!(
        (asked[0].method.as_str(), asked[0].path.as_str()),
        ("GET", SHOT_URL)
    );
    assert_eq!(
        asked[0].header("authorization"),
        Some(format!("Bearer {STORE_TOKEN}").as_str())
    );
    assert_eq!(asked[0].header("cf-access-client-id"), Some(ACCESS_ID));
    assert_eq!(
        asked[0].header("cf-access-client-secret"),
        Some(ACCESS_SECRET)
    );

    assert_eq!(get_file(&dir, "projects/P/raw/none.pdf").0, 404);
    assert_eq!(store.asked().len(), 2);

    let (status, _) = get_file(&dir, "projects/P/evidence/../../secret");
    assert!(matches!(status, 400 | 404), "{status}");
    assert_eq!(
        store.asked().len(),
        2,
        "the store is not asked for a bad path"
    );

    let peer = hub.raw(&format!(
        "GET /share/file?path=projects/P/evidence/t1/a%20b.png HTTP/1.1\r\nHost: x\r\n\
         Authorization: Bearer {PEER_TOKEN}\r\n\r\n"
    ));
    assert_eq!(status_of(&peer), 404, "{peer}");
    assert_eq!(store.asked().len(), 2);
}

#[test]
fn with_sharing_off_a_file_is_not_found() {
    let dir = TempDir::new("share-file-off");
    let bin = share_mem(&dir);
    let _hub = spawn_local(&dir, &bin, None);
    assert_eq!(get_file(&dir, "projects/P/evidence/t1/a%20b.png").0, 404);
}

fn route_on(dir: &TempDir) -> serde_json::Value {
    let response = on_socket(
        &dir.join("local.sock"),
        "GET /route HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    );
    let body = response.split("\r\n\r\n").nth(1).unwrap_or_default();
    serde_json::from_str(body).unwrap_or_default()
}

/// The hub web chooses the route through the store, which hands it to every
/// machine; the hub keeps it as it keeps a peer's, and mem never sees it.
#[test]
fn a_newer_route_from_the_store_is_taken_and_never_reaches_mem() {
    let dir = TempDir::new("share-route-in");
    let bin = share_mem(&dir);
    let store = FakeStore::start();
    let _hub = spawn_local(&dir, &bin, Some(&store));
    wait_for("a hello", Duration::from_secs(5), || {
        !store.texts_on(0).is_empty()
    });
    store.send(r#"{"t":"route","route":"device","machine":"nuc","at":4102444800000}"#);
    store.send(r#"{"t":"ready","seq":41}"#);
    wait_for("the route taken", Duration::from_secs(2), || {
        route_on(&dir)["machine"] == "nuc"
    });
    assert_eq!(route_on(&dir)["route"], "device");
    wait_for("the ready applied", Duration::from_secs(2), || {
        applied(&dir).contains("ready")
    });
    assert!(!applied(&dir).contains("route"), "{}", applied(&dir));
    // Taken from the store, so not sent back to it.
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(store.texts_on(0), [HELLO]);
}

/// A route chosen on this machine reaches the store, which keeps the newest
/// and shows it on the hub web.
#[test]
fn a_route_chosen_here_reaches_the_store() {
    let dir = TempDir::new("share-route-out");
    let bin = share_mem(&dir);
    let store = FakeStore::start();
    let _hub = spawn_local(&dir, &bin, Some(&store));
    wait_for("a hello", Duration::from_secs(5), || {
        !store.texts_on(0).is_empty()
    });
    let body = r#"{"route":"device"}"#;
    let chosen = on_socket(
        &dir.join("local.sock"),
        &format!(
            "POST /route HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
    );
    assert_eq!(status_of(&chosen), 200, "{chosen}");
    wait_for("the route sent", Duration::from_secs(2), || {
        store.texts_on(0).len() >= 2
    });
    let sent: serde_json::Value = serde_json::from_str(&store.texts_on(0)[1]).unwrap();
    assert_eq!(sent["t"], "route");
    assert_eq!(sent["route"], "device");
    assert_eq!(sent["machine"], route_on(&dir)["machine"]);
    assert_eq!(sent["at"], route_on(&dir)["at"]);
}

/// A `mem` like `share_mem` that also lists `projects.json` for
/// `projects --json`, and an `amx` that lists `amx.json`.
fn report_bins(dir: &TempDir) -> PathBuf {
    let bin = dir.join("bin");
    fixture_mem(
        &bin,
        &format!(
            "case \"$1 $2\" in\n\
             'share hello') echo '{HELLO}' ;;\n\
             'share pending') true ;;\n\
             'share apply') cat >> '{applied}' ;;\n\
             'projects --json') cat '{projects}' ;;\n\
             esac",
            applied = dir.join("applied").display(),
            projects = dir.join("projects.json").display(),
        ),
    );
    common::fixture_bin(
        &bin,
        "amx",
        &format!(
            "[ \"$1 $2\" = 'ls --json' ] && cat '{}'",
            dir.join("amx.json").display()
        ),
    );
    bin
}

fn git(dir: &Path, args: &[&str]) {
    let done = std::process::Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-C"])
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(done.status.success(), "git {args:?}: {done:?}");
}

/// A checkout whose main is two commits ahead of origin/main.
fn checkout_ahead(dir: &TempDir) -> PathBuf {
    let repo = dir.join("walk01");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    for n in 1..=3 {
        git(
            &repo,
            &["commit", "-q", "--allow-empty", "-m", &format!("c{n}")],
        );
    }
    git(&repo, &["update-ref", "refs/remotes/origin/main", "HEAD~2"]);
    repo
}

fn seed_machine(dir: &TempDir, agents: &str) {
    let repo = checkout_ahead(dir);
    std::fs::write(
        dir.join("projects.json"),
        serde_json::json!({"projects": [
            {"name": "walk01", "checkouts": [repo], "worktree": null},
            {"name": "walk02", "checkouts": [], "worktree": null},
            {"name": "walk03", "checkouts": [dir.join("nowhere")], "worktree": "/w/walk03"},
        ]})
        .to_string(),
    )
    .unwrap();
    std::fs::write(dir.join("amx.json"), agents).unwrap();
}

const RUNNING: &str = r#"[{"id":"walk01-m1","state":"running","ended":0,"created":5,"dir":"/x","last_words":"long text"}]"#;

fn reports_on(store: &FakeStore, conn: usize) -> Vec<serde_json::Value> {
    store
        .texts_on(conn)
        .iter()
        .filter_map(|t| serde_json::from_str::<serde_json::Value>(t).ok())
        .filter(|v| v["t"] == "report")
        .collect()
}

#[test]
fn a_hub_reports_its_machine_by_runner_name_after_the_hello() {
    let dir = TempDir::new("share-report");
    let bin = report_bins(&dir);
    seed_machine(&dir, RUNNING);
    let store = FakeStore::start();
    let _hub = spawn_local_env(&dir, &bin, Some(&store), &[("WORKFLOW_MACHINE", "mini")]);
    wait_for("a report", Duration::from_secs(5), || {
        !reports_on(&store, 0).is_empty()
    });
    assert_eq!(store.texts_on(0)[0], HELLO);
    assert_eq!(
        reports_on(&store, 0)[0],
        serde_json::json!({
            "t": "report",
            "machine": "mini",
            "agents": [{"id":"walk01-m1","state":"running","ended":0,"created":5}],
            "checkouts": ["walk01", "walk03"],
            "worktrees": ["walk03"],
            "unpushed": {"walk01": 2},
            "cloud": null,
        })
    );
}

#[test]
fn the_runner_name_is_the_tailnet_names_first_label() {
    let dir = TempDir::new("share-report-tailnet");
    let bin = report_bins(&dir);
    seed_machine(&dir, "[]");
    let store = FakeStore::start();
    let _hub = spawn_local_env(
        &dir,
        &bin,
        Some(&store),
        &[("HUB_TAILNET_NAME", "nuc.taila27604.ts.net")],
    );
    wait_for("a report", Duration::from_secs(5), || {
        !reports_on(&store, 0).is_empty()
    });
    assert_eq!(reports_on(&store, 0)[0]["machine"], "nuc");
}

/// 2,880 a day: an idle machine says the same again once per interval, and
/// no more often.
#[test]
fn an_idle_hub_reports_again_once_each_interval() {
    let dir = TempDir::new("share-report-idle");
    let bin = report_bins(&dir);
    seed_machine(&dir, RUNNING);
    let store = FakeStore::start();
    let _hub = spawn_local_env(
        &dir,
        &bin,
        Some(&store),
        &[
            ("WORKFLOW_MACHINE", "mini"),
            ("HUB_REPORT_EVERY_MS", "600"),
            ("HUB_REPORT_CHECK_MS", "50"),
        ],
    );
    wait_for("a report", Duration::from_secs(5), || {
        !reports_on(&store, 0).is_empty()
    });
    std::thread::sleep(Duration::from_millis(2000));
    let reports = reports_on(&store, 0);
    assert!(
        (3..=5).contains(&reports.len()),
        "{} reports in about 2 s at one per 600 ms",
        reports.len()
    );
    assert!(reports.iter().all(|r| *r == reports[0]));
}

#[test]
fn a_change_is_reported_at_the_next_check_not_the_next_interval() {
    let dir = TempDir::new("share-report-change");
    let bin = report_bins(&dir);
    seed_machine(&dir, RUNNING);
    let store = FakeStore::start();
    let _hub = spawn_local_env(
        &dir,
        &bin,
        Some(&store),
        &[
            ("WORKFLOW_MACHINE", "mini"),
            ("HUB_REPORT_EVERY_MS", "60000"),
            ("HUB_REPORT_CHECK_MS", "100"),
        ],
    );
    wait_for("a report", Duration::from_secs(5), || {
        !reports_on(&store, 0).is_empty()
    });
    std::fs::write(
        dir.join("amx.json"),
        r#"[{"id":"walk01-m1","state":"stopped","ended":9,"created":5}]"#,
    )
    .unwrap();
    let at = Instant::now();
    wait_for("the changed report", Duration::from_secs(3), || {
        reports_on(&store, 0).len() >= 2
    });
    assert!(at.elapsed() < Duration::from_secs(1), "{:?}", at.elapsed());
    assert_eq!(reports_on(&store, 0)[1]["agents"][0]["state"], "stopped");
}

#[test]
fn a_dropped_socket_reports_again_after_its_hello() {
    let dir = TempDir::new("share-report-again");
    let bin = report_bins(&dir);
    seed_machine(&dir, RUNNING);
    let store = FakeStore::start();
    let _hub = spawn_local_env(
        &dir,
        &bin,
        Some(&store),
        &[
            ("WORKFLOW_MACHINE", "mini"),
            ("HUB_REPORT_EVERY_MS", "60000"),
        ],
    );
    wait_for("a report", Duration::from_secs(5), || {
        !reports_on(&store, 0).is_empty()
    });
    store.drop_connection();
    wait_for("a report on the new socket", Duration::from_secs(8), || {
        !reports_on(&store, 1).is_empty()
    });
    assert_eq!(store.texts_on(1)[0], HELLO);
}

/// `report_bins`, plus a `systemd-run` and a `notify-send` that each append
/// their arguments to `ran`, one line a call, and a `workflow` that is never
/// to run outside them.
fn command_bins(dir: &TempDir) -> PathBuf {
    let bin = report_bins(dir);
    let ran = dir.join("ran");
    for name in ["systemd-run", "notify-send"] {
        common::fixture_bin(
            &bin,
            name,
            &format!("echo \"{name} $*\" >> '{}'", ran.display()),
        );
    }
    bin
}

fn ran(dir: &TempDir) -> Vec<String> {
    std::fs::read_to_string(dir.join("ran"))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

fn dones_on(store: &FakeStore, conn: usize) -> Vec<serde_json::Value> {
    store
        .texts_on(conn)
        .iter()
        .filter_map(|t| serde_json::from_str::<serde_json::Value>(t).ok())
        .filter(|v| v["t"] == "done")
        .collect()
}

fn cmd(id: &str, machine: &str, command: serde_json::Value) -> String {
    serde_json::json!({"t":"cmd","id":id,"machine":machine,"command":command}).to_string()
}

fn commanded_hub(dir: &TempDir, agents: &str, store: &FakeStore) -> Hub {
    let bin = command_bins(dir);
    seed_machine(dir, agents);
    let hub = spawn_local_env(dir, &bin, Some(store), &[("WORKFLOW_MACHINE", "mini")]);
    wait_for("a report", Duration::from_secs(5), || {
        !reports_on(store, 0).is_empty()
    });
    hub
}

#[test]
fn a_start_for_this_machine_runs_workflow_go_in_a_scope_of_its_own_then_says_done() {
    let dir = TempDir::new("share-cmd-start");
    let store = FakeStore::start();
    let _hub = commanded_hub(&dir, "[]", &store);
    let at = Instant::now();
    store.send(&cmd(
        "01CMDA",
        "mini",
        serde_json::json!({"kind":"start","project":"walk01"}),
    ));
    wait_for("the done", Duration::from_secs(5), || {
        !dones_on(&store, 0).is_empty()
    });
    assert!(at.elapsed() < Duration::from_secs(5));
    assert_eq!(
        ran(&dir),
        ["systemd-run --user --scope --quiet workflow go walk01"]
    );
    assert_eq!(
        dones_on(&store, 0),
        [serde_json::json!({"t":"done","id":"01CMDA","machine":"mini"})]
    );
}

#[test]
fn run_in_cloud_runs_workflow_go_cloud() {
    let dir = TempDir::new("share-cmd-cloud");
    let store = FakeStore::start();
    let _hub = commanded_hub(&dir, "[]", &store);
    store.send(&cmd(
        "01CMDC",
        "mini",
        serde_json::json!({"kind":"start","project":"walk01","cloud":true}),
    ));
    wait_for("the done", Duration::from_secs(5), || {
        !dones_on(&store, 0).is_empty()
    });
    assert_eq!(
        ran(&dir),
        ["systemd-run --user --scope --quiet workflow go walk01 --cloud"]
    );
}

/// The store sends a command again with each report until it hears done;
/// one already run is only answered again.
#[test]
fn a_command_heard_twice_runs_once_and_is_answered_each_time() {
    let dir = TempDir::new("share-cmd-twice");
    let store = FakeStore::start();
    let _hub = commanded_hub(&dir, "[]", &store);
    let start = cmd(
        "01CMDT",
        "mini",
        serde_json::json!({"kind":"start","project":"walk01"}),
    );
    store.send(&start);
    wait_for("the done", Duration::from_secs(5), || {
        !dones_on(&store, 0).is_empty()
    });
    store.send(&start);
    wait_for("the second done", Duration::from_secs(5), || {
        dones_on(&store, 0).len() >= 2
    });
    assert_eq!(ran(&dir).len(), 1, "{:?}", ran(&dir));
    // And after a restart of the hub too.
    store.drop_connection();
    wait_for("a new socket", Duration::from_secs(8), || {
        !reports_on(&store, 1).is_empty()
    });
    store.send(&start);
    wait_for("the done on it", Duration::from_secs(5), || {
        !dones_on(&store, 1).is_empty()
    });
    assert_eq!(ran(&dir).len(), 1, "{:?}", ran(&dir));
}

#[test]
fn a_command_for_another_machine_is_left_alone() {
    let dir = TempDir::new("share-cmd-other");
    let store = FakeStore::start();
    let _hub = commanded_hub(&dir, "[]", &store);
    store.send(&cmd(
        "01CMDN",
        "nuc",
        serde_json::json!({"kind":"start","project":"walk01"}),
    ));
    std::thread::sleep(Duration::from_millis(1500));
    assert!(ran(&dir).is_empty(), "{:?}", ran(&dir));
    assert!(dones_on(&store, 0).is_empty());
}

#[test]
fn a_resume_of_a_stopped_session_runs_amx_resume_with_the_message() {
    let dir = TempDir::new("share-cmd-resume");
    let store = FakeStore::start();
    let _hub = commanded_hub(
        &dir,
        r#"[{"id":"walk01-m1","state":"stopped","ended":9,"created":5}]"#,
        &store,
    );
    store.send(&cmd(
        "01CMDR",
        "mini",
        serde_json::json!({"kind":"resume","project":"walk01","id":"walk01-m1","message":"yes"}),
    ));
    wait_for("the done", Duration::from_secs(5), || {
        !dones_on(&store, 0).is_empty()
    });
    assert_eq!(
        ran(&dir),
        ["systemd-run --user --scope --quiet amx resume walk01-m1 yes"]
    );
}

#[test]
fn a_resume_of_a_session_amx_lost_starts_the_project_instead() {
    let dir = TempDir::new("share-cmd-lost");
    let store = FakeStore::start();
    let _hub = commanded_hub(&dir, "[]", &store);
    store.send(&cmd(
        "01CMDL",
        "mini",
        serde_json::json!({"kind":"resume","project":"walk01","id":"walk01-m1","message":"yes"}),
    ));
    wait_for("the done", Duration::from_secs(5), || {
        !dones_on(&store, 0).is_empty()
    });
    assert_eq!(
        ran(&dir),
        ["systemd-run --user --scope --quiet workflow go walk01"]
    );
}

#[test]
fn a_resume_of_a_live_session_runs_nothing() {
    let dir = TempDir::new("share-cmd-live");
    let store = FakeStore::start();
    let _hub = commanded_hub(&dir, RUNNING, &store);
    store.send(&cmd(
        "01CMDV",
        "mini",
        serde_json::json!({"kind":"resume","project":"walk01","id":"walk01-m1","message":"yes"}),
    ));
    wait_for("the done", Duration::from_secs(5), || {
        !dones_on(&store, 0).is_empty()
    });
    assert!(ran(&dir).is_empty(), "{:?}", ran(&dir));
}

/// A cloud VM has no amx, and its tailnet name is ephemeral: it reports as
/// its session, which its runner record names.
#[test]
fn a_cloud_session_reports_itself_as_its_session() {
    let dir = TempDir::new("share-report-cloud");
    let bin = report_bins(&dir);
    seed_machine(&dir, "[]");
    let store = FakeStore::start();
    let _hub = spawn_local_env(
        &dir,
        &bin,
        Some(&store),
        &[("CLAUDE_CODE_REMOTE_SESSION_ID", "cse_01VPKGED")],
    );
    wait_for("a report", Duration::from_secs(5), || {
        !reports_on(&store, 0).is_empty()
    });
    let report = &reports_on(&store, 0)[0];
    assert_eq!(report["machine"], "session_01VPKGED");
    assert_eq!(report["cloud"], "session_01VPKGED");
}

#[test]
fn a_cloud_session_without_amx_still_reports() {
    let dir = TempDir::new("share-report-cloud-noamx");
    let bin = report_bins(&dir);
    seed_machine(&dir, "[]");
    std::fs::remove_file(bin.join("amx")).unwrap();
    let store = FakeStore::start();
    let _hub = spawn_local_env(
        &dir,
        &bin,
        Some(&store),
        &[("CLAUDE_CODE_REMOTE_SESSION_ID", "cse_01VPKGED")],
    );
    wait_for("a report", Duration::from_secs(5), || {
        !reports_on(&store, 0).is_empty()
    });
    assert_eq!(reports_on(&store, 0)[0]["agents"], serde_json::json!([]));
}
