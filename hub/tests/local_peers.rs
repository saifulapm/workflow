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
    Hub::spawn_env(
        &home,
        &path,
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
    let mini = spawn(&dir, "mini", &[&nuc], Some(&bin));
    ask(&socket_of(&dir, "nuc"), PROBE);
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
    let mini = spawn(&dir, "mini", &[&nuc], None);
    ask(&socket_of(&dir, "nuc"), PROBE);
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
