//! `hub share seed` against a fake store: the hello asks for every write,
//! the plan mem prints goes out as imports, and a `budget` answer ends the
//! run with exit 3 before any file is put.

mod common;

use std::net::TcpListener;
use std::process::Command;
use std::sync::{Arc, Mutex};

use common::{TempDir, fixture_mem, hub_bin};
use tungstenite::Message;
use tungstenite::handshake::server::{Request, Response};

/// What the fake store kept: every text frame, and the store object asked for.
type Kept = (Arc<Mutex<Vec<String>>>, Arc<Mutex<Option<String>>>);

const HELLO: &str = r#"{"t":"hello","machine":"testbox","since":41}"#;

/// A store that answers the hello with one rec and `ready`, then acks the
/// first `acks` writes and answers `budget` to the rest; it keeps every
/// text frame it got and the X-Hub-Store it was asked for.
#[allow(clippy::result_large_err)]
fn fake_store(acks: usize) -> (String, Kept) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}/store/share", listener.local_addr().unwrap());
    let frames = Arc::new(Mutex::new(Vec::new()));
    let object = Arc::new(Mutex::new(None));
    let (kept, named) = (Arc::clone(&frames), Arc::clone(&object));
    std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut socket = tungstenite::accept_hdr(stream, |req: &Request, res: Response| {
            *named.lock().unwrap() = req
                .headers()
                .get("X-Hub-Store")
                .map(|v| v.to_str().unwrap().to_string());
            Ok(res)
        })
        .unwrap();
        let mut writes = 0;
        while let Ok(message) = socket.read() {
            let Message::Text(text) = message else {
                continue;
            };
            kept.lock().unwrap().push(text.to_string());
            let frame: serde_json::Value = serde_json::from_str(&text).unwrap();
            let replies = match frame["t"].as_str() {
                Some("hello") => vec![
                    r#"{"t":"rec","seq":1,"id":"01J0000000000000000000REC1","path":"projects/P/items/a.md","body":"a","machine":"nuc"}"#.to_string(),
                    r#"{"t":"ready","seq":1,"rows":89000}"#.to_string(),
                ],
                Some("write") if writes < acks => {
                    writes += 1;
                    vec![format!(r#"{{"t":"ack","id":{},"seq":{}}}"#, frame["id"], writes + 1)]
                }
                Some("write") => vec![format!(
                    r#"{{"t":"budget","id":{},"rows":90003}}"#,
                    frame["id"]
                )],
                _ => vec![],
            };
            for reply in replies {
                socket.send(Message::text(reply)).unwrap();
            }
        }
    });
    (url, (frames, object))
}

#[test]
fn a_budget_answer_ends_the_import_with_exit_3_before_any_file() {
    let dir = TempDir::new("seed-budget");
    let bin = dir.join("bin");
    let plan: String = (1..=5)
        .map(|n| {
            format!(
                r#"{{"t":"write","id":"01J000000000000000000WRIT{n}","path":"projects/P/items/{n}.md","base":null,"body":"{n}","import":true}}"#
            ) + "\n"
        })
        .collect::<String>()
        + r#"{"t":"kept","path":"projects/P/wiki/lessons.md"}"#
        + "\n"
        + r#"{"t":"file","path":"projects/P/evidence/x.png","local":"/nonexistent"}"#;
    std::fs::write(dir.join("plan"), format!("{plan}\n")).unwrap();
    fixture_mem(
        &bin,
        &format!(
            "case \"$1 $2\" in\n\
             'share hello') echo '{HELLO}' ;;\n\
             'share seed') cat > '{replay}'; cat '{plan}' ;;\n\
             'share seeded') cat >> '{seeded}' ;;\n\
             esac",
            replay = dir.join("replay").display(),
            plan = dir.join("plan").display(),
            seeded = dir.join("seeded").display(),
        ),
    );
    let (url, (frames, object)) = fake_store(2);
    let out = Command::new(hub_bin())
        .args(["share", "seed"])
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("HUB_STORE_URL", &url)
        .env("HUB_STORE_TOKEN", "store-token-for-tests")
        .env("HUB_STORE_NAME", "main")
        .env_remove("HUB_STORE_ACCESS_ID")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(3),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("sent 2 of 5 writes"), "{stdout}");
    assert!(stdout.contains("answered budget"), "{stdout}");
    assert!(stdout.contains("after 06:00 Dhaka"), "{stdout}");
    assert!(
        stdout.contains("1 page another machine wrote is kept here as .conflict-import"),
        "{stdout}"
    );

    let frames = frames.lock().unwrap().clone();
    let hello: serde_json::Value = serde_json::from_str(&frames[0]).unwrap();
    assert_eq!(hello["since"], 0, "the import asks for every write");
    assert_eq!(hello["machine"], "testbox");
    let writes: Vec<serde_json::Value> = frames[1..]
        .iter()
        .map(|f| serde_json::from_str(f).unwrap())
        .collect();
    assert_eq!(writes.len(), 5);
    assert!(writes.iter().all(|w| w["import"] == true));
    assert_eq!(object.lock().unwrap().as_deref(), Some("main"));
    let replay = std::fs::read_to_string(dir.join("replay")).unwrap();
    assert!(
        replay.contains("REC1"),
        "mem plans against the store's replay: {replay}"
    );
    assert!(
        !dir.join("seeded").exists(),
        "no file is put once the budget is spent"
    );
}
