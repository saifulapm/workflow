//! A person's question rings its route: a Slack post from the asking hub on
//! Phone, the route device's popup on Device, and Phone again whenever the
//! route device does not take it.

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{Hub, TempDir, status_of};
use serde_json::Value;

const TOKEN: &str = "peer-token-for-tests-0123456789abcdef";

/// A Slack Web API that records each `chat.postMessage` and `chat.update`
/// and refuses the rest,
/// Socket Mode included, which the hub then retries quietly.
struct Slack {
    api: String,
    posts: Arc<Mutex<Vec<String>>>,
    updates: Arc<Mutex<Vec<String>>>,
}

fn slack() -> Slack {
    slack_on(TcpListener::bind("127.0.0.1:0").unwrap())
}

fn slack_on(listener: TcpListener) -> Slack {
    let api = format!("http://{}/api", listener.local_addr().unwrap());
    let posts = Arc::new(Mutex::new(Vec::new()));
    let kept = Arc::clone(&posts);
    let updates = Arc::new(Mutex::new(Vec::new()));
    let kept_updates = Arc::clone(&updates);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            let mut length = 0;
            loop {
                let mut header = String::new();
                if reader.read_line(&mut header).unwrap_or(0) == 0 {
                    break;
                }
                let header = header.trim_end();
                if header.is_empty() {
                    break;
                }
                if let Some((name, value)) = header.split_once(':')
                    && name.eq_ignore_ascii_case("content-length")
                {
                    length = value.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0; length];
            let _ = reader.read_exact(&mut body);
            let reply = if line.contains("/api/chat.postMessage") {
                kept.lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&body).to_string());
                r#"{"ok":true,"channel":"D0DM","ts":"1700000000.000100"}"#
            } else if line.contains("/api/chat.update") {
                kept_updates
                    .lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&body).to_string());
                r#"{"ok":true}"#
            } else {
                r#"{"ok":false,"error":"not_in_this_fake"}"#
            };
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{reply}",
                reply.len()
            );
        }
    });
    Slack {
        api,
        posts,
        updates,
    }
}

impl Slack {
    fn posted(&self) -> usize {
        self.posts.lock().unwrap().len()
    }
}

/// How a machine's session looks to presence.
#[derive(Clone, Copy)]
enum Seat {
    /// A live shell, unlocked, not idle.
    Watching,
    /// A live shell, locked this long ago.
    LockedFor(u64),
    /// No shell has ever run here (the nuc).
    NoShell,
}

struct Machine {
    hub: Hub,
    home: PathBuf,
}

impl Machine {
    fn host(&self) -> String {
        format!("127.0.0.1:{}", self.hub.port)
    }

    fn socket(&self) -> PathBuf {
        self.home.join("local.sock")
    }

    /// The questions in its file for qshell.
    fn listed(&self) -> Vec<Value> {
        std::fs::read(self.home.join("run/hub-questions.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|d| d["questions"].as_array().cloned())
            .unwrap_or_default()
    }

    fn popups(&self) -> usize {
        self.listed().iter().filter(|q| q["popup"] == true).count()
    }
}

fn machine(
    dir: &TempDir,
    name: &str,
    seat: Seat,
    peers: &[&str],
    slack: Option<&Slack>,
    extra: &[(&str, &str)],
) -> Machine {
    machine_at(dir, name, seat, peers, slack, extra, 0)
}

fn machine_at(
    dir: &TempDir,
    name: &str,
    seat: Seat,
    peers: &[&str],
    slack: Option<&Slack>,
    extra: &[(&str, &str)],
    port: u16,
) -> Machine {
    let home = dir.join(name);
    std::fs::create_dir_all(home.join("config/qshell")).unwrap();
    std::fs::create_dir_all(home.join("run")).unwrap();
    std::fs::write(home.join("config/qshell/machine"), format!("{name}\n")).unwrap();
    let bin = dir.join(&format!("{name}-bin"));
    std::fs::create_dir_all(&bin).unwrap();
    common::fixture_mem(
        &bin,
        "case \"$*\" in *questions*) echo '{\"questions\":[]}';; esac",
    );
    match seat {
        Seat::Watching | Seat::LockedFor(_) => {
            common::fixture_bin(&bin, "pgrep", "exit 0");
            std::fs::create_dir_all(home.join("state/qshell")).unwrap();
        }
        Seat::NoShell => {
            common::fixture_bin(&bin, "pgrep", "exit 1");
        }
    }
    if let Seat::LockedFor(secs) = seat {
        let locked = home.join("state/qshell/locked");
        std::fs::write(&locked, "").unwrap();
        let at = std::time::SystemTime::now() - Duration::from_secs(secs);
        std::fs::File::options()
            .write(true)
            .open(&locked)
            .unwrap()
            .set_modified(at)
            .unwrap();
    }
    let socket = home.join("local.sock");
    let runtime = home.join("run");
    let peers = peers.join(",");
    let mut env = vec![
        ("HUB_PEER_TOKEN", TOKEN),
        ("HUB_LOCAL_SOCKET", socket.to_str().unwrap()),
        ("HUB_HOST", "127.0.0.1"),
        ("HUB_PEERS", &peers),
        ("XDG_RUNTIME_DIR", runtime.to_str().unwrap()),
    ];
    if let Some(slack) = slack {
        env.extend([
            ("SLACK_BOT_TOKEN", "xoxb-fake"),
            ("SLACK_APP_TOKEN", "xapp-fake"),
            ("SLACK_USER_ID", "U0SAIFUL"),
            ("SLACK_API_URL", slack.api.as_str()),
        ]);
    }
    env.extend_from_slice(extra);
    let port = port.to_string();
    let hub = Hub::spawn_env(&home, &[&bin], &["local", "--port", &port], &env);
    Machine { hub, home }
}

fn on_socket(socket: &Path, request: &str) -> String {
    let mut stream = UnixStream::connect(socket).unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

fn post(socket: &Path, path: &str, body: &str) {
    let response = on_socket(
        socket,
        &format!(
            "POST {path} HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ),
    );
    assert_eq!(status_of(&response), 200, "{path}: {response}");
}

fn route_device(on: &Machine, device: &str) {
    post(
        &on.socket(),
        "/route",
        &format!(r#"{{"route":"device","machine":"{device}"}}"#),
    );
}

const PROBE: &str = r#"{"id":"ABCD1234","project":"walk","text":"probe?","options":["yes","no"],"audience":"human"}"#;

fn ask(on: &Machine) {
    post(&on.socket(), "/ask", PROBE);
}

fn quiet_for(what: &str, time: Duration, f: impl Fn() -> bool) {
    let until = std::time::Instant::now() + time;
    while std::time::Instant::now() < until {
        assert!(f(), "{what}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn on_phone_the_asking_hub_posts_to_slack_and_no_popup_shows() {
    let dir = TempDir::new("ring-phone");
    let slack = slack();
    let nuc = machine(&dir, "nuc", Seat::NoShell, &[], Some(&slack), &[]);
    ask(&nuc);
    common::wait_for("a Slack post", Duration::from_secs(2), || {
        slack.posted() == 1
    });
    assert_eq!(nuc.popups(), 0);
}

#[test]
fn on_device_a_watching_route_device_shows_the_popup_and_slack_stays_quiet() {
    let dir = TempDir::new("ring-device");
    let slack = slack();
    let mini = machine(&dir, "mini", Seat::Watching, &[], None, &[]);
    let nuc = machine(
        &dir,
        "nuc",
        Seat::NoShell,
        &[&mini.host()],
        Some(&slack),
        &[],
    );
    route_device(&nuc, &mini.host());
    ask(&nuc);
    common::wait_for("the mini's popup", Duration::from_secs(2), || {
        mini.popups() == 1
    });
    assert_eq!(mini.listed()[0]["machine"], nuc.host());
    assert_eq!(nuc.popups(), 0);
    quiet_for("no Slack post", Duration::from_millis(800), || {
        slack.posted() == 0
    });
    assert_eq!(mini.popups(), 1);
}

#[test]
fn a_route_device_locked_a_minute_still_takes_the_question() {
    let dir = TempDir::new("ring-locked-minute");
    let slack = slack();
    let mini = machine(&dir, "mini", Seat::LockedFor(60), &[], None, &[]);
    let nuc = machine(
        &dir,
        "nuc",
        Seat::NoShell,
        &[&mini.host()],
        Some(&slack),
        &[],
    );
    route_device(&nuc, &mini.host());
    ask(&nuc);
    common::wait_for("the mini's popup", Duration::from_secs(2), || {
        mini.popups() == 1
    });
    quiet_for("no Slack post", Duration::from_millis(500), || {
        slack.posted() == 0
    });
}

#[test]
fn a_route_device_locked_five_minutes_sends_the_question_to_phone_at_once() {
    let dir = TempDir::new("ring-locked");
    let slack = slack();
    let mini = machine(&dir, "mini", Seat::LockedFor(6 * 60), &[], None, &[]);
    let nuc = machine(
        &dir,
        "nuc",
        Seat::NoShell,
        &[&mini.host()],
        Some(&slack),
        &[],
    );
    route_device(&nuc, &mini.host());
    ask(&nuc);
    common::wait_for("a Slack post", Duration::from_secs(2), || {
        slack.posted() == 1
    });
    // The mini still counts it, but never pops it up.
    common::wait_for("the mini counts it", Duration::from_secs(2), || {
        mini.listed().len() == 1
    });
    quiet_for("no popup on the mini", Duration::from_millis(500), || {
        mini.popups() == 0
    });
}

#[test]
fn a_route_device_that_is_offline_sends_the_question_to_phone_at_once() {
    let dir = TempDir::new("ring-offline");
    let slack = slack();
    let nuc = machine(&dir, "nuc", Seat::NoShell, &[], Some(&slack), &[]);
    // The macbook is off: the tailnet does not list it.
    route_device(&nuc, "macbook.example.ts.net");
    ask(&nuc);
    common::wait_for("a Slack post", Duration::from_secs(2), || {
        slack.posted() == 1
    });
}

#[test]
fn a_route_device_that_never_answers_sends_the_question_to_phone_in_time() {
    let dir = TempDir::new("ring-silent");
    let slack = slack();
    // Listed on the tailnet, but its hub takes the connection and says nothing.
    let silent = TcpListener::bind("127.0.0.1:0").unwrap();
    let silent_host = format!("127.0.0.1:{}", silent.local_addr().unwrap().port());
    let _held = std::thread::spawn(move || {
        let mut held = Vec::new();
        for stream in silent.incoming() {
            held.push(stream);
        }
    });
    let nuc = machine(
        &dir,
        "nuc",
        Seat::NoShell,
        &[&silent_host],
        Some(&slack),
        &[("HUB_RING_MS", "400")],
    );
    route_device(&nuc, &silent_host);
    let started = std::time::Instant::now();
    ask(&nuc);
    common::wait_for("a Slack post", Duration::from_secs(3), || {
        slack.posted() == 1
    });
    assert!(
        started.elapsed() >= Duration::from_millis(350),
        "{:?}",
        started.elapsed()
    );
}

/// The asking hub gave up on the route device before its answer came: the
/// question is on Phone now, and the device must not pop it up after all,
/// whichever of the two messages reaches it first.
#[test]
fn a_question_gone_to_phone_never_shows_as_a_popup_later() {
    let dir = TempDir::new("ring-late");
    let mini = machine(&dir, "mini", Seat::Watching, &[], None, &[]);
    let peer = |path: &str, body: &str| {
        mini.hub.raw(&format!(
            "POST {path} HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {TOKEN}\r\n\
             Content-Length: {}\r\n\r\n{body}",
            body.len()
        ))
    };
    let open = |id: &str, ring: &str| {
        format!(
            r#"{{"id":"{id}","project":"walk","machine":"nuc","text":"probe?","options":["yes"],"person":true,"ring":{ring}}}"#
        )
    };

    // The offer comes late, after Phone was decided.
    assert_eq!(
        status_of(&peer("/question", &open("LATE0001", r#""phone""#))),
        200
    );
    let late = peer("/ring", &open("LATE0001", "null"));
    assert_eq!(status_of(&late), 409, "{late}");
    assert_eq!(mini.popups(), 0);

    // The offer was taken, then Phone was decided after all.
    let taken = peer("/ring", &open("LATE0002", "null"));
    assert_eq!(status_of(&taken), 200, "{taken}");
    assert_eq!(mini.popups(), 1);
    assert_eq!(
        status_of(&peer("/question", &open("LATE0002", r#""phone""#))),
        200
    );
    assert_eq!(mini.popups(), 0);

    // The first news of the question, arriving last, keeps it on Phone.
    assert_eq!(
        status_of(&peer("/question", &open("LATE0002", "null"))),
        200
    );
    assert_eq!(mini.popups(), 0);

    // Nor can it take down a popup the route device rightly shows.
    assert_eq!(status_of(&peer("/ring", &open("LATE0003", "null"))), 200);
    assert_eq!(
        status_of(&peer("/question", &open("LATE0003", "null"))),
        200
    );
    assert_eq!(mini.popups(), 1);
    assert_eq!(mini.listed().len(), 3);
}

#[test]
fn on_device_here_a_watching_asker_pops_up_its_own_question() {
    let dir = TempDir::new("ring-self");
    let slack = slack();
    let mini = machine(&dir, "mini", Seat::Watching, &[], Some(&slack), &[]);
    post(&mini.socket(), "/route", r#"{"route":"device"}"#);
    ask(&mini);
    common::wait_for("the popup here", Duration::from_secs(2), || {
        mini.popups() == 1
    });
    quiet_for("no Slack post", Duration::from_millis(500), || {
        slack.posted() == 0
    });
}

#[test]
fn on_device_here_with_no_shell_the_question_goes_to_phone() {
    let dir = TempDir::new("ring-self-away");
    let slack = slack();
    let nuc = machine(&dir, "nuc", Seat::NoShell, &[], Some(&slack), &[]);
    post(&nuc.socket(), "/route", r#"{"route":"device"}"#);
    ask(&nuc);
    common::wait_for("a Slack post", Duration::from_secs(2), || {
        slack.posted() == 1
    });
    assert_eq!(nuc.popups(), 0);
}

#[test]
fn an_orchestrator_question_and_a_roadmap_review_ring_nowhere() {
    let dir = TempDir::new("ring-none");
    let slack = slack();
    let mini = machine(&dir, "mini", Seat::Watching, &[], None, &[]);
    let nuc = machine(
        &dir,
        "nuc",
        Seat::NoShell,
        &[&mini.host()],
        Some(&slack),
        &[],
    );
    post(
        &nuc.socket(),
        "/ask",
        r#"{"id":"ORCH0001","project":"walk","text":"which?","options":["a","b"],"audience":"orchestrator"}"#,
    );
    post(
        &nuc.socket(),
        "/ask",
        r#"{"id":"REVW0001","project":"walk","text":"Review the move roadmap","options":["approve","change"],"audience":"human"}"#,
    );
    common::wait_for("the mini counts both", Duration::from_secs(2), || {
        mini.listed().len() == 2
    });
    route_device(&nuc, &mini.host());
    post(
        &nuc.socket(),
        "/ask",
        r#"{"id":"ORCH0002","project":"walk","text":"which?","options":[],"audience":"orchestrator"}"#,
    );
    common::wait_for("the mini counts three", Duration::from_secs(2), || {
        mini.listed().len() == 3
    });
    quiet_for("nothing rings", Duration::from_millis(800), || {
        slack.posted() == 0 && mini.popups() == 0
    });
}

/// No network: Slack cannot be reached when the question is asked, and the
/// post goes out soon after it can.
#[test]
fn a_question_asked_offline_reaches_slack_once_the_network_returns() {
    let dir = TempDir::new("ring-offline-slack");
    let port = {
        let probe = TcpListener::bind("127.0.0.1:0").unwrap();
        probe.local_addr().unwrap().port()
    };
    let api = format!("http://127.0.0.1:{port}/api");
    let away = Slack {
        api,
        posts: Arc::new(Mutex::new(Vec::new())),
        updates: Arc::new(Mutex::new(Vec::new())),
    };
    let nuc = machine(
        &dir,
        "nuc",
        Seat::NoShell,
        &[],
        Some(&away),
        &[("HUB_RETRY_MS", "200")],
    );
    ask(&nuc);
    std::thread::sleep(Duration::from_millis(700));
    let back = slack_on(TcpListener::bind(("127.0.0.1", port)).unwrap());
    common::wait_for(
        "the post once Slack is back",
        Duration::from_secs(2),
        || back.posted() == 1,
    );
    quiet_for("one post only", Duration::from_millis(600), || {
        back.posted() == 1
    });
}

/// A peer that was unreachable when the question was asked hears it once it
/// is back, so its bar counts it. Unreachable is a connection that goes
/// nowhere, here one cut at once; a refused one is a machine with no hub.
#[test]
fn a_peer_that_was_away_hears_the_question_when_it_returns() {
    let dir = TempDir::new("ring-offline-peer");
    let cutter = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = cutter.local_addr().unwrap().port();
    cutter.set_nonblocking(true).unwrap();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cutting = {
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                if let Ok((stream, _)) = cutter.accept() {
                    drop(stream);
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        })
    };
    let later = format!("127.0.0.1:{port}");
    let nuc = machine(
        &dir,
        "nuc",
        Seat::NoShell,
        &[&later],
        None,
        &[("HUB_RETRY_MS", "200")],
    );
    ask(&nuc);
    std::thread::sleep(Duration::from_millis(700));
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    cutting.join().unwrap();
    let mini = machine_at(&dir, "mini", Seat::NoShell, &[], None, &[], port);
    common::wait_for(
        "the mini hears it once back",
        Duration::from_secs(2),
        || mini.listed().len() == 1,
    );
}

/// Answered anywhere else, the question's Slack message loses its buttons,
/// so the DM never shows it open.
#[test]
fn a_question_answered_elsewhere_is_marked_answered_in_slack() {
    let dir = TempDir::new("ring-mark");
    let slack = slack();
    let nuc = machine(&dir, "nuc", Seat::NoShell, &[], Some(&slack), &[]);
    ask(&nuc);
    common::wait_for("a Slack post", Duration::from_secs(2), || {
        slack.posted() == 1
    });
    post(
        &nuc.socket(),
        "/answer",
        r#"{"question":"ABCD1234","answer":"no","by":"cli"}"#,
    );
    common::wait_for(
        "the message marked answered",
        Duration::from_secs(2),
        || !slack.updates.lock().unwrap().is_empty(),
    );
    let update: Value = serde_json::from_str(&slack.updates.lock().unwrap()[0]).unwrap();
    assert_eq!(update["ts"], "1700000000.000100");
    assert!(
        update["text"].as_str().unwrap().ends_with("Answered: no"),
        "{update}"
    );
}

/// Cloudflare unreachable on every machine: the store's socket fails, and a
/// question still rings Slack, pops up on the route device and has its
/// answer carried back, all inside the targets.
#[test]
fn questions_flow_with_the_store_unreachable() {
    let dir = TempDir::new("ring-no-store");
    let blocked = [
        ("HUB_STORE_URL", "ws://127.0.0.1:9/store/share"),
        ("HUB_STORE_TOKEN", "store-token-for-tests"),
    ];
    let slack = slack();
    let mini = machine(&dir, "mini", Seat::Watching, &[], None, &blocked);
    let nuc = machine(
        &dir,
        "nuc",
        Seat::NoShell,
        &[&mini.host()],
        Some(&slack),
        &blocked,
    );

    // Phone: the asking hub posts to Slack itself.
    ask(&nuc);
    common::wait_for("a Slack post", Duration::from_secs(2), || {
        slack.posted() == 1
    });
    post(
        &nuc.socket(),
        "/answer",
        r#"{"question":"ABCD1234","answer":"yes","by":"cli"}"#,
    );

    // Device: the popup on the mini, and its answer back on the nuc.
    route_device(&nuc, &mini.host());
    post(
        &nuc.socket(),
        "/ask",
        r#"{"id":"EFGH5678","project":"walk","text":"popup?","options":["ok"],"audience":"human"}"#,
    );
    common::wait_for("the mini's popup", Duration::from_secs(2), || {
        mini.popups() == 1
    });
    let waiter = {
        let socket = nuc.socket();
        std::thread::spawn(move || {
            let started = std::time::Instant::now();
            let got = on_socket(
                &socket,
                "GET /wait/EFGH5678?s=10 HTTP/1.1\r\nHost: x\r\n\r\n",
            );
            (got, started.elapsed())
        })
    };
    std::thread::sleep(Duration::from_millis(200));
    post(
        &mini.socket(),
        "/answer",
        r#"{"question":"EFGH5678","answer":"ok","by":"popup"}"#,
    );
    let (got, took) = waiter.join().unwrap();
    assert_eq!(common::body_of(&got), "ok", "{got}");
    assert!(took < Duration::from_secs(2), "{took:?}");
}
