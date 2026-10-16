//! Local hubs tell each other about every question, so each bar's waiting
//! count is right on every machine.

mod common;

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use common::{Hub, TempDir, status_of};
use serde_json::Value;

const TOKEN: &str = "peer-token-for-tests-0123456789abcdef";

/// A local hub named `machine` in its own corner of `dir`, reachable by
/// peers at 127.0.0.1, with `peers` as its tailnet.
fn spawn(dir: &TempDir, machine: &str, peers: &[&Hub], bin: Option<&Path>) -> Hub {
    spawn_on(dir, machine, peers, bin, 0)
}

/// `spawn` on `port`: a restarted hub keeps its name.
fn spawn_on(dir: &TempDir, machine: &str, peers: &[&Hub], bin: Option<&Path>, port: u16) -> Hub {
    let home = dir.join(machine);
    std::fs::create_dir_all(home.join("config/qshell")).unwrap();
    std::fs::create_dir_all(home.join("run")).unwrap();
    std::fs::write(home.join("config/qshell/machine"), format!("{machine}\n")).unwrap();
    let socket = home.join("local.sock");
    let runtime = home.join("run");
    let peers = peers
        .iter()
        .map(|p| format!("127.0.0.1:{}", p.port))
        .collect::<Vec<_>>()
        .join(",");
    let path: Vec<&Path> = bin.into_iter().collect();
    let port = port.to_string();
    Hub::spawn_env(
        &home,
        &path,
        &["local", "--port", &port],
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

fn questions_file(dir: &TempDir, machine: &str) -> PathBuf {
    dir.join(machine).join("run/hub-questions.json")
}

/// The questions in a hub's file for qshell, or none while it has none.
fn listed(dir: &TempDir, machine: &str) -> Vec<Value> {
    std::fs::read(questions_file(dir, machine))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|doc| doc["questions"].as_array().cloned())
        .unwrap_or_default()
}

fn on_socket(socket: &Path, request: &str) -> String {
    let mut stream = UnixStream::connect(socket).unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

fn ask(socket: &Path, body: &str) {
    let asked = on_socket(
        socket,
        &format!(
            "POST /ask HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ),
    );
    assert_eq!(status_of(&asked), 200, "{asked}");
}

fn answer(hub: &Hub, body: &str) -> String {
    hub.raw(&format!(
        "POST /answer HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {TOKEN}\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    ))
}

/// A mem that answers every `questions` with none answered and keeps quiet
/// about the rest.
fn quiet_mem(dir: &TempDir) -> PathBuf {
    let bin = dir.join("bin");
    common::fixture_mem(
        &bin,
        "case \"$*\" in *questions*) echo '{\"questions\":[]}';; esac",
    );
    bin
}

const PROBE: &str = r#"{"id":"ABCD1234","project":"walk","text":"probe?","options":["yes","a b"],"audience":"human"}"#;

#[test]
fn a_question_asked_on_one_hub_reaches_its_peers_and_leaves_them_once_answered() {
    let dir = TempDir::new("peers-hear");
    let bin = quiet_mem(&dir);
    let mini = spawn(&dir, "mini", &[], Some(&bin));
    let nuc = spawn(&dir, "nuc", &[&mini], Some(&bin));

    ask(&socket_of(&dir, "nuc"), PROBE);
    common::wait_for(
        "the mini hears the nuc's question",
        Duration::from_secs(2),
        || listed(&dir, "mini").len() == 1,
    );
    let heard = &listed(&dir, "mini")[0];
    assert_eq!(heard["id"], "ABCD1234");
    assert_eq!(heard["project"], "walk");
    assert_eq!(heard["machine"], format!("127.0.0.1:{}", nuc.port));
    assert_eq!(heard["text"], "probe?");
    assert_eq!(heard["options"], serde_json::json!(["yes", "a b"]));
    assert_eq!(heard["person"], true);
    assert_eq!(heard["popup"], false);
    // The asking hub lists its own question too, for its own bar.
    assert_eq!(listed(&dir, "nuc").len(), 1);

    let answered = answer(
        &nuc,
        r#"{"question":"ABCD1234","answer":"yes","by":"slack"}"#,
    );
    assert_eq!(status_of(&answered), 200, "{answered}");
    common::wait_for(
        "the mini drops the answered question",
        Duration::from_secs(2),
        || listed(&dir, "mini").is_empty(),
    );
    assert!(listed(&dir, "nuc").is_empty());
}

#[test]
fn the_file_for_qshell_names_this_hub() {
    let dir = TempDir::new("peers-self");
    let mini = spawn(&dir, "mini", &[], None);
    common::wait_for(
        "the file is written at start",
        Duration::from_secs(2),
        || questions_file(&dir, "mini").exists(),
    );
    let doc: Value =
        serde_json::from_slice(&std::fs::read(questions_file(&dir, "mini")).unwrap()).unwrap();
    assert_eq!(doc["self"], format!("127.0.0.1:{}", mini.port));
    assert_eq!(doc["questions"], serde_json::json!([]));
}

#[test]
fn an_orchestrator_question_is_heard_but_not_counted_as_a_person_question() {
    let dir = TempDir::new("peers-orch");
    let mini = spawn(&dir, "mini", &[], None);
    let _nuc = spawn(&dir, "nuc", &[&mini], None);
    ask(
        &socket_of(&dir, "nuc"),
        r#"{"id":"ORCH0001","project":"walk","text":"which?","options":[],"audience":"orchestrator"}"#,
    );
    common::wait_for("the mini hears it", Duration::from_secs(2), || {
        listed(&dir, "mini").len() == 1
    });
    assert_eq!(listed(&dir, "mini")[0]["person"], false);
}

#[test]
fn a_hub_that_starts_late_learns_the_open_questions_from_its_peers() {
    let dir = TempDir::new("peers-late");
    let nuc = spawn(&dir, "nuc", &[], None);
    ask(&socket_of(&dir, "nuc"), PROBE);
    let _mini = spawn(&dir, "mini", &[&nuc], None);
    common::wait_for(
        "the late mini lists the nuc's question",
        Duration::from_secs(2),
        || listed(&dir, "mini").len() == 1,
    );
    assert_eq!(listed(&dir, "mini")[0]["id"], "ABCD1234");
}

/// A question that was answered while this hub was down must not come back
/// from its own saved copy: the asking hub's list is the truth for its own.
#[test]
fn a_restarted_hub_forgets_a_question_its_asker_no_longer_lists() {
    let dir = TempDir::new("peers-restart");
    let bin = quiet_mem(&dir);
    let nuc = spawn(&dir, "nuc", &[], Some(&bin));
    ask(&socket_of(&dir, "nuc"), PROBE);
    let mini = spawn(&dir, "mini", &[&nuc], Some(&bin));
    common::wait_for("the mini learns it", Duration::from_secs(3), || {
        listed(&dir, "mini").len() == 1
    });
    drop(mini);
    let answered = answer(
        &nuc,
        r#"{"question":"ABCD1234","answer":"yes","by":"slack"}"#,
    );
    assert_eq!(status_of(&answered), 200, "{answered}");
    let _mini = spawn(&dir, "mini", &[&nuc], Some(&bin));
    common::wait_for(
        "the restarted mini has nothing open",
        Duration::from_secs(2),
        || questions_file(&dir, "mini").exists() && listed(&dir, "mini").is_empty(),
    );
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        listed(&dir, "mini").is_empty(),
        "{:?}",
        listed(&dir, "mini")
    );
}

/// A saved question survives a restart while its asker cannot be reached.
#[test]
fn a_restarted_hub_keeps_what_it_knew_while_the_asker_is_away() {
    let dir = TempDir::new("peers-keep");
    let nuc = spawn(&dir, "nuc", &[], None);
    ask(&socket_of(&dir, "nuc"), PROBE);
    let mini = spawn(&dir, "mini", &[&nuc], None);
    common::wait_for("the mini learns it", Duration::from_secs(3), || {
        listed(&dir, "mini").len() == 1
    });
    drop(mini);
    drop(nuc);
    std::fs::remove_file(questions_file(&dir, "mini")).unwrap();
    let _mini = spawn(&dir, "mini", &[], None);
    common::wait_for(
        "the restarted mini lists it again",
        Duration::from_secs(2),
        || listed(&dir, "mini").len() == 1,
    );
}

#[test]
fn a_question_from_a_hub_without_the_token_is_refused() {
    let dir = TempDir::new("peers-forged");
    let mini = spawn(&dir, "mini", &[], None);
    let body =
        r#"{"id":"EVIL0001","project":"walk","machine":"x","text":"t","options":[],"person":true}"#;
    let forged = mini.raw(&format!(
        "POST /question HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer nope\r\n\
         Content-Length: {}\r\n\r\n{body}",
        body.len()
    ));
    assert_eq!(status_of(&forged), 401, "{forged}");
    std::thread::sleep(Duration::from_millis(200));
    assert!(listed(&dir, "mini").is_empty());
}

fn route_on(socket: &Path, body: &str) -> String {
    on_socket(
        socket,
        &format!(
            "POST /route HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ),
    )
}

fn route_of(socket: &Path) -> Value {
    let response = on_socket(socket, "GET /route HTTP/1.1\r\nHost: x\r\n\r\n");
    assert_eq!(status_of(&response), 200, "{response}");
    serde_json::from_str(common::body_of(&response)).unwrap()
}

#[test]
fn a_route_set_on_one_hub_reaches_its_peers() {
    let dir = TempDir::new("route-spread");
    let nuc = spawn(&dir, "nuc", &[], None);
    let mini = spawn(&dir, "mini", &[&nuc], None);
    assert_eq!(route_of(&socket_of(&dir, "mini"))["route"], "phone");

    // Device with no machine named is Device here, by this hub's own host.
    let set = route_on(&socket_of(&dir, "mini"), r#"{"route":"device"}"#);
    assert_eq!(status_of(&set), 200, "{set}");
    let here = route_of(&socket_of(&dir, "mini"));
    assert_eq!(here["route"], "device");
    assert_eq!(here["machine"], format!("127.0.0.1:{}", mini.port));
    common::wait_for("the nuc hears the route", Duration::from_secs(2), || {
        route_of(&socket_of(&dir, "nuc"))["route"] == "device"
    });
    assert_eq!(
        route_of(&socket_of(&dir, "nuc"))["machine"],
        here["machine"]
    );

    let phone = route_on(&socket_of(&dir, "mini"), r#"{"route":"phone"}"#);
    assert_eq!(status_of(&phone), 200, "{phone}");
    common::wait_for("the nuc hears Phone", Duration::from_secs(2), || {
        route_of(&socket_of(&dir, "nuc"))["route"] == "phone"
    });
}

#[test]
fn an_older_route_from_a_peer_loses_to_the_newer_one_here() {
    let dir = TempDir::new("route-older");
    let mini = spawn(&dir, "mini", &[], None);
    route_on(&socket_of(&dir, "mini"), r#"{"route":"device"}"#);
    let stale = r#"{"route":"phone","machine":null,"at":1}"#;
    let response = mini.raw(&format!(
        "POST /route HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {TOKEN}\r\n\
         Content-Length: {}\r\n\r\n{stale}",
        stale.len()
    ));
    assert_eq!(status_of(&response), 200, "{response}");
    assert_eq!(route_of(&socket_of(&dir, "mini"))["route"], "device");
}

#[test]
fn a_hub_keeps_its_route_across_a_restart_and_a_late_one_learns_the_newest() {
    let dir = TempDir::new("route-late");
    let nuc = spawn(&dir, "nuc", &[], None);
    route_on(&socket_of(&dir, "nuc"), r#"{"route":"device"}"#);
    let machine = route_of(&socket_of(&dir, "nuc"))["machine"].clone();
    drop(nuc);
    let nuc = spawn(&dir, "nuc", &[], None);
    let kept = route_of(&socket_of(&dir, "nuc"));
    assert_eq!(kept["route"], "device");
    assert_eq!(kept["machine"], machine);

    let _mini = spawn(&dir, "mini", &[&nuc], None);
    common::wait_for(
        "the late mini learns Device",
        Duration::from_secs(2),
        || route_of(&socket_of(&dir, "mini"))["route"] == "device",
    );
}

#[test]
fn a_route_that_is_neither_phone_nor_device_is_refused() {
    let dir = TempDir::new("route-bad");
    let _mini = spawn(&dir, "mini", &[], None);
    for body in [r#"{"route":"dulali"}"#, r#"{"route":"pager"}"#, "nonsense"] {
        let response = route_on(&socket_of(&dir, "mini"), body);
        assert_eq!(status_of(&response), 400, "{body}: {response}");
    }
    assert_eq!(route_of(&socket_of(&dir, "mini"))["route"], "phone");
}

/// A mem that lists no answered questions and logs every other call, each
/// as a `direct=<MEM_ANSWER_DIRECT>` line then its argv, copying a
/// `--screenshot` file to `shot.png` beside the log while it still exists.
fn answering_mem(dir: &TempDir, machine: &str) -> (PathBuf, PathBuf) {
    let bin = dir.join(&format!("{machine}-bin"));
    let log = dir.join(&format!("{machine}-mem.log"));
    let shot = dir.join(&format!("{machine}-shot.png"));
    common::fixture_mem(
        &bin,
        &format!(
            "case \"$*\" in *questions*) echo '{{\"questions\":[]}}'; exit 0;; esac\n\
             {{ echo \"direct=$MEM_ANSWER_DIRECT\"; printf '%s\\n' \"$@\"; }} >> '{log}'\n\
             prev=\n\
             for a in \"$@\"; do [ \"$prev\" = --screenshot ] && cp \"$a\" '{shot}'; prev=$a; done\n\
             exit 0",
            log = log.display(),
            shot = shot.display(),
        ),
    );
    (bin, log)
}

fn answer_on(socket: &Path, body: &str) -> String {
    on_socket(
        socket,
        &format!(
            "POST /answer HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ),
    )
}

fn wait_on(socket: &Path, id: &str, secs: u64) -> (String, Duration) {
    let started = std::time::Instant::now();
    let response = on_socket(
        socket,
        &format!("GET /wait/{id}?s={secs} HTTP/1.1\r\nHost: x\r\n\r\n"),
    );
    (response, started.elapsed())
}

/// The nuc asks, the mini hears it: the pair most answer tests start from.
fn asked_on_the_nuc(dir: &TempDir) -> (Hub, Hub, PathBuf, PathBuf) {
    let (nuc_bin, nuc_log) = answering_mem(dir, "nuc");
    let (mini_bin, mini_log) = answering_mem(dir, "mini");
    let mini = spawn(dir, "mini", &[], Some(&mini_bin));
    let nuc = spawn(dir, "nuc", &[&mini], Some(&nuc_bin));
    ask(&socket_of(dir, "nuc"), PROBE);
    common::wait_for("the mini hears it", Duration::from_secs(2), || {
        listed(dir, "mini").len() == 1
    });
    (nuc, mini, nuc_log, mini_log)
}

#[test]
fn an_answer_given_on_another_machine_wakes_the_asker_and_is_recorded_there() {
    let dir = TempDir::new("answer-elsewhere");
    let (_nuc, _mini, nuc_log, mini_log) = asked_on_the_nuc(&dir);
    let waiter = {
        let socket = socket_of(&dir, "nuc");
        std::thread::spawn(move || wait_on(&socket, "ABCD1234", 10))
    };
    std::thread::sleep(Duration::from_millis(200));
    let given = answer_on(
        &socket_of(&dir, "mini"),
        r#"{"question":"ABCD1234","answer":"use hash","by":"cli"}"#,
    );
    assert_eq!(status_of(&given), 200, "{given}");
    let (response, took) = waiter.join().unwrap();
    assert_eq!(common::body_of(&response), "use hash");
    assert!(took < Duration::from_secs(2), "{took:?}");

    common::wait_for("the nuc records it", Duration::from_secs(5), || {
        std::fs::read_to_string(&nuc_log).is_ok_and(|l| l.contains("use hash"))
    });
    let logged = std::fs::read_to_string(&nuc_log).unwrap();
    let lines: Vec<&str> = logged.lines().collect();
    // Written straight to the store: mem must not hand it back to the hub.
    assert_eq!(
        lines,
        [
            "direct=1", "answer", "--by", "cli", "--", "ABCD1234", "use hash"
        ]
    );
    assert!(
        !mini_log.exists(),
        "{:?}",
        std::fs::read_to_string(&mini_log)
    );
    common::wait_for("the mini drops it", Duration::from_secs(2), || {
        listed(&dir, "mini").is_empty()
    });
}

#[test]
fn a_popup_answer_carries_its_screenshot_to_the_asking_machine() {
    let dir = TempDir::new("answer-shot");
    let (_nuc, _mini, nuc_log, _) = asked_on_the_nuc(&dir);
    let png: Vec<u8> = (0..200_000u32).map(|i| (i * 7 % 251) as u8).collect();
    let file = dir.join("grabbed.png");
    std::fs::write(&file, &png).unwrap();
    let body = serde_json::json!({
        "question": "ABCD1234",
        "answer": "ok",
        "by": "popup",
        "screenshot": file.to_str().unwrap(),
    })
    .to_string();
    let given = answer_on(&socket_of(&dir, "mini"), &body);
    assert_eq!(status_of(&given), 200, "{given}");
    common::wait_for("the nuc records it", Duration::from_secs(5), || {
        dir.join("nuc-shot.png").exists()
    });
    let logged = std::fs::read_to_string(&nuc_log).unwrap();
    let lines: Vec<&str> = logged.lines().collect();
    assert_eq!(&lines[..4], ["direct=1", "answer", "--by", "popup"]);
    assert_eq!(lines[4], "--screenshot");
    assert_eq!(&lines[6..], ["--", "ABCD1234", "ok"]);
    assert_eq!(std::fs::read(dir.join("nuc-shot.png")).unwrap(), png);
}

#[test]
fn the_first_answer_stands_whichever_machine_gives_the_next() {
    let dir = TempDir::new("answer-first");
    let (nuc, _mini, nuc_log, _) = asked_on_the_nuc(&dir);
    let first = answer_on(
        &socket_of(&dir, "mini"),
        r#"{"question":"ABCD1234","answer":"yes","by":"cli"}"#,
    );
    assert_eq!(status_of(&first), 200, "{first}");
    let here = answer_on(
        &socket_of(&dir, "nuc"),
        r#"{"question":"ABCD1234","answer":"no","by":"cli"}"#,
    );
    assert_eq!(status_of(&here), 409, "{here}");
    assert_eq!(common::body_of(&here).trim(), "yes");
    let tapped = answer(
        &nuc,
        r#"{"question":"ABCD1234","answer":"no","by":"slack"}"#,
    );
    assert_eq!(status_of(&tapped), 409, "{tapped}");
    std::thread::sleep(Duration::from_millis(300));
    let logged = std::fs::read_to_string(&nuc_log).unwrap();
    assert_eq!(logged.matches("answer\n").count(), 1, "{logged}");
    assert!(!logged.contains("no\n"), "{logged}");
}

#[test]
fn a_roadmap_review_answered_from_another_machine_is_refused() {
    let dir = TempDir::new("answer-review");
    let (nuc_bin, nuc_log) = answering_mem(&dir, "nuc");
    let mini = spawn(&dir, "mini", &[], None);
    let _nuc = spawn(&dir, "nuc", &[&mini], Some(&nuc_bin));
    ask(
        &socket_of(&dir, "nuc"),
        r#"{"id":"REVW0001","project":"walk","text":"Review the move roadmap","options":["approve","change"],"audience":"human"}"#,
    );
    common::wait_for("the mini hears it", Duration::from_secs(2), || {
        listed(&dir, "mini").len() == 1
    });
    let given = answer_on(
        &socket_of(&dir, "mini"),
        r#"{"question":"REVW0001","answer":"approve","by":"cli"}"#,
    );
    assert_eq!(status_of(&given), 403, "{given}");
    assert!(common::body_of(&given).contains("Saiful"), "{given}");
    std::thread::sleep(Duration::from_millis(300));
    assert!(!nuc_log.exists());
}

#[test]
fn an_answer_to_a_question_no_hub_here_knows_is_left_to_mem() {
    let dir = TempDir::new("answer-unknown");
    let _mini = spawn(&dir, "mini", &[], None);
    let given = answer_on(
        &socket_of(&dir, "mini"),
        r#"{"question":"ZZZZ9999","answer":"yes","by":"cli"}"#,
    );
    assert_eq!(status_of(&given), 404, "{given}");
}

/// An answer that reaches the store another way (mem answer written
/// straight, the hub web through sharing) ends the wait early, so mem
/// looks at its store again at once.
#[test]
fn a_write_to_the_store_ends_a_wait_early() {
    let dir = TempDir::new("answer-kick");
    let _mini = spawn(&dir, "mini", &[], None);
    let waiter = {
        let socket = socket_of(&dir, "mini");
        std::thread::spawn(move || wait_on(&socket, "ABCD1234", 10))
    };
    std::thread::sleep(Duration::from_millis(300));
    let kicked = on_socket(
        &socket_of(&dir, "mini"),
        "POST /share/kick HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\n\r\n",
    );
    assert_eq!(status_of(&kicked), 204, "{kicked}");
    let (response, took) = waiter.join().unwrap();
    assert_eq!(status_of(&response), 204, "{response}");
    assert!(took < Duration::from_secs(2), "{took:?}");
}

/// An answer that reached the asking machine's store without passing this
/// hub (written straight, or from the hub web) still closes the question
/// everywhere, so no bar counts it for ever.
#[test]
fn a_question_answered_in_the_store_is_closed_on_every_hub() {
    let dir = TempDir::new("answer-swept");
    let flag = dir.join("answered");
    let bin = dir.join("nuc-bin");
    common::fixture_mem(
        &bin,
        &format!(
            "case \"$*\" in *questions*) if [ -e '{flag}' ]; then \
             echo '{{\"questions\":[{{\"short_id\":\"ABCD1234\",\"answered\":true,\"answer\":\"yes\"}}]}}'; \
             else echo '{{\"questions\":[]}}'; fi;; esac",
            flag = flag.display()
        ),
    );
    let mini = spawn(&dir, "mini", &[], None);
    let _nuc = spawn_swept(&dir, "nuc", &[&mini], &bin);
    ask(&socket_of(&dir, "nuc"), PROBE);
    common::wait_for("the mini hears it", Duration::from_secs(2), || {
        listed(&dir, "mini").len() == 1
    });
    std::fs::write(&flag, "").unwrap();
    common::wait_for("the mini drops it", Duration::from_secs(2), || {
        listed(&dir, "mini").is_empty()
    });
    assert!(listed(&dir, "nuc").is_empty());
    let (response, _) = wait_on(&socket_of(&dir, "nuc"), "ABCD1234", 1);
    assert_eq!(common::body_of(&response), "yes");
}

/// `spawn`, sweeping its own open questions every 200 ms.
fn spawn_swept(dir: &TempDir, machine: &str, peers: &[&Hub], bin: &Path) -> Hub {
    let home = dir.join(machine);
    std::fs::create_dir_all(home.join("config/qshell")).unwrap();
    std::fs::create_dir_all(home.join("run")).unwrap();
    std::fs::write(home.join("config/qshell/machine"), format!("{machine}\n")).unwrap();
    let socket = home.join("local.sock");
    let runtime = home.join("run");
    let peers = peers
        .iter()
        .map(|p| format!("127.0.0.1:{}", p.port))
        .collect::<Vec<_>>()
        .join(",");
    Hub::spawn_env(
        &home,
        &[bin],
        &["local", "--port", "0"],
        &[
            ("HUB_PEER_TOKEN", TOKEN),
            ("HUB_LOCAL_SOCKET", socket.to_str().unwrap()),
            ("HUB_HOST", "127.0.0.1"),
            ("HUB_PEERS", &peers),
            ("XDG_RUNTIME_DIR", runtime.to_str().unwrap()),
            ("HUB_SWEEP_MS", "200"),
        ],
    )
}

/// A machine that slept while a question was asked and answered (the asker
/// never reached it, since the tailnet listed it offline) catches up within
/// a sweep once it is back: it counts what it missed, then drops it once
/// answered.
#[test]
fn a_machine_that_was_asleep_catches_up_within_a_sweep() {
    let dir = TempDir::new("peers-asleep");
    let bin = quiet_mem(&dir);
    let nuc = spawn(&dir, "nuc", &[], Some(&bin));
    let _mini = spawn_swept(&dir, "mini", &[&nuc], &bin);
    std::thread::sleep(Duration::from_millis(300));
    ask(&socket_of(&dir, "nuc"), PROBE);
    common::wait_for(
        "the mini catches up on the question",
        Duration::from_secs(2),
        || listed(&dir, "mini").len() == 1,
    );
    let answered = answer(
        &nuc,
        r#"{"question":"ABCD1234","answer":"yes","by":"slack"}"#,
    );
    assert_eq!(status_of(&answered), 200, "{answered}");
    common::wait_for(
        "the mini drops the answered question",
        Duration::from_secs(2),
        || listed(&dir, "mini").is_empty(),
    );
}

/// Only the asking hub speaks for its own questions: a peer cannot rewrite
/// this hub's copy, which after a restart is all it has to tell a roadmap
/// review by.
#[test]
fn a_peer_cannot_rewrite_a_question_this_hub_asked() {
    let dir = TempDir::new("peers-own");
    let bin = quiet_mem(&dir);
    let nuc = spawn(&dir, "nuc", &[], Some(&bin));
    ask(
        &socket_of(&dir, "nuc"),
        r#"{"id":"REVW0001","project":"walk","text":"Review the move roadmap","options":["approve","change"],"audience":"human"}"#,
    );
    let host = format!("127.0.0.1:{}", nuc.port);
    let forged = serde_json::json!({
        "id": "REVW0001", "project": "walk", "machine": host,
        "text": "harmless", "options": ["approve"], "person": true,
    })
    .to_string();
    let response = nuc.raw(&format!(
        "POST /question HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {TOKEN}\r\n\
         Content-Length: {}\r\n\r\n{forged}",
        forged.len()
    ));
    assert_eq!(status_of(&response), 200, "{response}");
    assert_eq!(listed(&dir, "nuc")[0]["text"], "Review the move roadmap");

    let port = nuc.port;
    drop(nuc);
    let nuc = spawn_on(&dir, "nuc", &[], Some(&bin), port);
    let tapped = answer(
        &nuc,
        r#"{"question":"REVW0001","answer":"approve","by":"slack"}"#,
    );
    assert_eq!(status_of(&tapped), 403, "{tapped}");
}

/// An answer for a question this hub never asked has nowhere to go: a tap
/// on an old message, after its asker restarted, must not read as taken.
#[test]
fn a_peer_answer_to_a_question_this_hub_did_not_ask_is_not_found() {
    let dir = TempDir::new("peers-unknown");
    let mini = spawn(&dir, "mini", &[], None);
    let response = answer(
        &mini,
        r#"{"question":"ZZZZ9999","answer":"yes","by":"slack"}"#,
    );
    assert_eq!(status_of(&response), 404, "{response}");
}
