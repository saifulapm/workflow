//! `hub local` holds this machine's sharing socket: a fake store on
//! 127.0.0.1 and a fixture `mem` stand in for Cloudflare and the replica.

mod common;

use std::io::{Read, Write};
use std::net::TcpListener;
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
const WRITE_B: &str = r#"{"t":"write","id":"01J00000000000000000000BBB","path":"projects/P/wiki/b.md","base":null,"body":"b"}"#;

/// What the fake store saw, connection by connection.
#[derive(Default)]
struct Seen {
    /// The Authorization header of each connection, in order.
    auth: Vec<Option<String>>,
    /// Each text frame, with the index of the connection it came on.
    texts: Vec<(usize, String)>,
}

enum Order {
    Send(String),
    Drop,
}

/// A store that takes one connection at a time, records what arrives and
/// sends or drops on order.
struct FakeStore {
    url: String,
    seen: Arc<Mutex<Seen>>,
    orders: Sender<Order>,
}

impl FakeStore {
    fn start() -> FakeStore {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("ws://{}/store/share", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Seen::default()));
        let (orders, rx) = channel();
        let recorded = Arc::clone(&seen);
        std::thread::spawn(move || serve(listener, recorded, rx));
        FakeStore { url, seen, orders }
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

// tungstenite fixes the header callback's error type, a whole Response.
#[allow(clippy::result_large_err)]
fn serve(listener: TcpListener, seen: Arc<Mutex<Seen>>, orders: Receiver<Order>) {
    for (conn, stream) in listener.incoming().enumerate() {
        let Ok(stream) = stream else { continue };
        let auth = Arc::clone(&seen);
        let socket =
            tungstenite::accept_hdr(stream, move |request: &Request, response: Response| {
                let header = request
                    .headers()
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
                auth.lock().unwrap().auth.push(header);
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
                Ok(_) => {}
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
    Hub::spawn_env(&home, &[bin], &["local", "--port", "0"], &env)
}

fn on_socket(socket: &Path, request: &str) -> String {
    let mut stream = UnixStream::connect(socket).unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
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
