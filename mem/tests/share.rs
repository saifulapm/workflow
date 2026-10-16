//! Sharing: a write to a shared project waits in this machine's outbox until
//! the store acknowledges it, and `mem share apply` lands the store's writes in
//! the replica, turning a refused page into a conflict file.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use common::{World, code, mem, mem_env, stderr, stdout};
use serde_json::{Value, json};

const ID: &str = "01K0SHARE0000000000000000P";

fn shared(tag: &str) -> World {
    let w = World::new(tag);
    w.project(ID, "walk");
    let out = mem(
        &w,
        w.dir.path(),
        &["--project", "walk", "project", "set", "share"],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    w
}

/// Runs mem with `input` on stdin.
fn mem_in(w: &World, args: &[&str], input: &str) -> Output {
    let dirs = w.dirs();
    let mut child = Command::new(env!("CARGO_BIN_EXE_mem"))
        .current_dir(w.dir.path())
        .args(args)
        .env("XDG_DATA_HOME", &dirs.data)
        .env("XDG_CACHE_HOME", &dirs.cache)
        .env("XDG_STATE_HOME", &dirs.state)
        .env("XDG_CONFIG_HOME", &dirs.config)
        .env("PI_CODING_AGENT_DIR", w.pi_agent_dir())
        .env("MEM_SYNC_CMD", "true")
        .env_remove("MEM_PROJECT")
        .env_remove("HUB_LOCAL_SOCKET")
        .env_remove("CARGO_TARGET_DIR")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run mem");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn walk(w: &World, args: &[&str]) -> Output {
    let mut all = vec!["--project", "walk"];
    all.extend_from_slice(args);
    let out = mem(w, w.dir.path(), &all);
    assert_eq!(code(&out), 0, "mem {args:?}: {}", stderr(&out));
    out
}

fn wiki_set(w: &World, slug: &str, text: &str) {
    let out = mem_in(
        w,
        &[
            "--project",
            "walk",
            "wiki",
            slug,
            "--stdin",
            "--note",
            "edit",
        ],
        text,
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
}

fn pending(w: &World) -> Vec<Value> {
    let out = mem(w, w.dir.path(), &["share", "pending"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    stdout(&out)
        .lines()
        .map(|line| serde_json::from_str(line).expect("one JSON write per line"))
        .collect()
}

fn apply(w: &World, messages: &[Value]) {
    let input: String = messages.iter().map(|m| format!("{m}\n")).collect();
    let out = mem_in(w, &["share", "apply"], &input);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
}

fn hello(w: &World) -> Value {
    let out = mem(w, w.dir.path(), &["share", "hello"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    serde_json::from_str(stdout(&out).trim()).expect("hello is JSON")
}

fn page_path(slug: &str) -> String {
    format!("projects/{ID}/wiki/{slug}.md")
}

fn wiki(w: &World, slug: &str) -> String {
    stdout(&walk(w, &["wiki", slug]))
}

#[test]
fn a_log_in_a_shared_project_waits_in_the_outbox() {
    let w = shared("share-log");
    let out = walk(&w, &["log", "probe from the mini", "--json"]);
    let item: Value = serde_json::from_slice(&out.stdout).unwrap();
    let file = item["path"].as_str().unwrap();

    let writes = pending(&w);
    assert_eq!(writes.len(), 1, "{writes:?}");
    let write = &writes[0];
    assert_eq!(write["t"], "write");
    let store = w.dirs().store();
    let rel = Path::new(file).strip_prefix(&store).unwrap();
    assert_eq!(write["path"], rel.to_str().unwrap());
    assert!(
        write["path"]
            .as_str()
            .unwrap()
            .starts_with(&format!("projects/{ID}/items/"))
    );
    assert_eq!(write["base"], Value::Null);
    assert_eq!(write["body"], std::fs::read_to_string(file).unwrap());
    assert!(
        write["body"]
            .as_str()
            .unwrap()
            .contains("probe from the mini")
    );
    assert_eq!(write["id"].as_str().unwrap().len(), 26);
}

#[test]
fn a_project_that_does_not_share_leaves_the_outbox_empty() {
    let w = World::new("share-off");
    w.project(ID, "walk");
    walk(&w, &["log", "stays here"]);
    assert!(pending(&w).is_empty());
}

#[test]
fn a_second_edit_of_a_page_is_based_on_the_first() {
    let w = shared("share-base");
    wiki_set(&w, "lessons", "# Lessons\n\none\n");
    wiki_set(&w, "lessons", "# Lessons\n\none\ntwo\n");

    let pages: Vec<Value> = pending(&w)
        .into_iter()
        .filter(|write| write["path"] == page_path("lessons"))
        .collect();
    assert_eq!(pages.len(), 2, "{pages:?}");
    assert_eq!(pages[0]["base"], Value::Null);
    assert_eq!(pages[1]["base"], pages[0]["id"]);
    assert_eq!(pages[1]["body"], "# Lessons\n\none\ntwo\n");
}

#[test]
fn an_ack_takes_the_write_out_of_the_outbox_and_raises_the_sequence() {
    let w = shared("share-ack");
    walk(&w, &["log", "one"]);
    walk(&w, &["log", "two"]);
    let writes = pending(&w);
    assert_eq!(hello(&w)["since"], 0);

    apply(&w, &[json!({"t": "ack", "id": writes[0]["id"], "seq": 7})]);

    let left = pending(&w);
    assert_eq!(left.len(), 1);
    assert_eq!(left[0]["id"], writes[1]["id"]);
    let said = hello(&w);
    assert_eq!(said["t"], "hello");
    assert_eq!(said["since"], 7);
    assert!(said["machine"].as_str().is_some_and(|m| !m.is_empty()));
}

#[test]
fn a_streamed_write_lands_in_the_replica_and_is_not_sent_back() {
    let w = shared("share-rec");
    apply(
        &w,
        &[
            json!({"t": "rec", "seq": 3, "id": "01K0SHAREREC00000000000001", "path": page_path("notes"),
                   "body": "# Notes\n\nfrom the nuc\n", "machine": "nuc"}),
            json!({"t": "ready", "seq": 5}),
        ],
    );

    assert_eq!(wiki(&w, "notes"), "# Notes\n\nfrom the nuc\n");
    assert!(pending(&w).is_empty(), "a streamed write went back out");
    assert_eq!(hello(&w)["since"], 5);

    // The next local edit builds on the streamed one.
    wiki_set(&w, "notes", "# Notes\n\nfrom the nuc\nand the mini\n");
    let page = pending(&w)
        .into_iter()
        .find(|write| write["path"] == page_path("notes"))
        .unwrap();
    assert_eq!(page["base"], "01K0SHAREREC00000000000001");
}

#[test]
fn a_streamed_item_is_listed_by_mem_log() {
    let w = shared("share-item");
    let mut item = common::item(
        mem::item::Kind::Log,
        "probe from the nuc",
        "probe from the nuc\n",
    );
    item.meta.id = "01K0SHARE7EM00000000000001".to_string();
    item.meta.project = Some("walk".to_string());
    let body = String::from_utf8(item.to_bytes().unwrap()).unwrap();
    apply(
        &w,
        &[
            json!({"t": "rec", "seq": 1, "id": "01K0SHAREW00000000000000001",
                 "path": format!("projects/{ID}/items/01K0SHARE7EM00000000000001.md"),
                 "body": body, "machine": "nuc"}),
        ],
    );
    assert!(stdout(&walk(&w, &["log"])).contains("probe from the nuc"));
}

#[test]
fn a_streamed_write_never_overwrites_a_local_edit_still_waiting() {
    let w = shared("share-pending-wins");
    wiki_set(&w, "lessons", "# Lessons\n\nmine\n");
    apply(
        &w,
        &[
            json!({"t": "rec", "seq": 2, "id": "01K0SHAREREC00000000000002", "path": page_path("lessons"),
                 "body": "# Lessons\n\ntheirs\n", "machine": "nuc"}),
        ],
    );
    assert_eq!(wiki(&w, "lessons"), "# Lessons\n\nmine\n");
}

#[test]
fn a_refused_page_becomes_a_conflict_file_and_the_stores_version_wins() {
    let w = shared("share-refused");
    wiki_set(&w, "lessons", "# Lessons\n\nmine\n");
    let page = pending(&w)
        .into_iter()
        .find(|write| write["path"] == page_path("lessons"))
        .unwrap();

    apply(
        &w,
        &[json!({"t": "refused", "id": page["id"],
                 "head": {"seq": 9, "id": "01K0SHAREHEAD0000000000001", "path": page_path("lessons"),
                          "body": "# Lessons\n\ntheirs\n"}})],
    );

    assert_eq!(wiki(&w, "lessons"), "# Lessons\n\ntheirs\n");
    assert!(pending(&w).iter().all(|write| write["id"] != page["id"]));
    let dir = w.dirs().store().join(format!("projects/{ID}/wiki"));
    let conflicts: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.to_string_lossy().contains(".conflict"))
        .collect();
    assert_eq!(conflicts.len(), 1, "{conflicts:?}");
    assert_eq!(
        std::fs::read_to_string(&conflicts[0]).unwrap(),
        "# Lessons\n\nmine\n"
    );
    let doctor = stdout(&mem(&w, w.dir.path(), &["doctor"]));
    assert!(
        doctor.contains(&conflicts[0].to_string_lossy().to_string()),
        "{doctor}"
    );
}

#[test]
fn doctor_reports_the_outbox_and_the_sequence() {
    let w = shared("share-doctor");
    for n in 1..=5 {
        walk(&w, &["log", &format!("offline {n}")]);
    }
    let doctor = stdout(&mem(&w, w.dir.path(), &["doctor"]));
    assert!(doctor.contains("sharing"), "{doctor}");
    assert!(doctor.contains("outbox 5"), "{doctor}");

    let ids: Vec<Value> = pending(&w)
        .iter()
        .map(|write| write["id"].clone())
        .collect();
    let acks: Vec<Value> = ids
        .iter()
        .zip(1..)
        .map(|(id, seq)| json!({"t": "ack", "id": id, "seq": seq}))
        .collect();
    apply(&w, &acks);
    let doctor = stdout(&mem(&w, w.dir.path(), &["doctor"]));
    assert!(doctor.contains("seq 5, outbox 0"), "{doctor}");
}

#[test]
fn a_write_killed_before_its_replica_rename_is_sent_and_healed_on_its_ack() {
    let w = shared("share-killed");
    let store = w.dirs().store();
    let file = store.join(page_path("killed"));
    // What a mem killed between the outbox and the replica leaves behind: the
    // outbox row, and no file.
    mem::share::queue(
        &w.dirs().share_db(),
        &store,
        &file,
        Some(b"# Killed\n\nstill here\n"),
    )
    .unwrap();
    assert!(!file.exists());

    let writes = pending(&w);
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0]["body"], "# Killed\n\nstill here\n");

    apply(&w, &[json!({"t": "ack", "id": writes[0]["id"], "seq": 1})]);
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "# Killed\n\nstill here\n"
    );
}

#[test]
fn clearing_the_plan_shares_a_removal() {
    let w = shared("share-clear");
    let out = mem_in(
        &w,
        &["--project", "walk", "plan", "--stdin"],
        "# plan: m1\n\n- [ ] t1 a\n",
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    walk(&w, &["plan", "--clear"]);
    let plan: Vec<Value> = pending(&w)
        .into_iter()
        .filter(|write| write["path"] == format!("projects/{ID}/plan.md"))
        .collect();
    assert_eq!(plan.len(), 2, "{plan:?}");
    assert_eq!(plan[1]["body"], Value::Null);
    assert_eq!(plan[1]["base"], plan[0]["id"]);

    apply(
        &w,
        &[
            json!({"t": "rec", "seq": 4, "id": "01K0SHAREREC00000000000004", "path": page_path("gone"),
                 "body": "# Gone\n", "machine": "nuc"}),
            json!({"t": "rec", "seq": 5, "id": "01K0SHAREREC00000000000005", "path": page_path("gone"),
                 "body": null, "machine": "nuc"}),
        ],
    );
    assert!(!w.dirs().store().join(page_path("gone")).exists());
}

#[test]
fn a_streamed_path_outside_the_projects_is_refused() {
    let w = shared("share-escape");
    let out = mem_in(
        &w,
        &["share", "apply"],
        &format!(
            "{}\n",
            json!({"t": "rec", "seq": 1, "id": "01K0SHAREREC00000000000009",
                   "path": "projects/../../../escaped.md", "body": "x", "machine": "nuc"})
        ),
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(!w.dir.path().join("escaped.md").exists());
    assert!(!w.dirs().data.join("escaped.md").exists());
    assert_eq!(hello(&w)["since"], 0);
}

#[test]
fn a_shared_write_kicks_the_local_hub() {
    let w = shared("share-kick");
    let socket = w.dir.join("hub.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let seen = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(_) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(_) => return String::from("no kick"),
            }
        };
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut line = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut line)
            .unwrap();
        let _ = (&stream).write_all(b"HTTP/1.1 204 No Content\r\n\r\n");
        line
    });
    let out = mem_env(
        &w,
        w.dir.path(),
        &["--project", "walk", "log", "kick it"],
        &[("HUB_LOCAL_SOCKET", socket.to_str().unwrap())],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(seen.join().unwrap().trim(), "POST /share/kick HTTP/1.1");
}

#[test]
fn evidence_in_a_shared_project_waits_to_be_uploaded() {
    let w = shared("share-evidence");
    let shot = w.dir.join("shot.png");
    std::fs::write(&shot, [0u8, 159, 146, 150, 255]).unwrap();
    walk(
        &w,
        &[
            "evidence",
            "add",
            "--task",
            "t1",
            shot.to_str().unwrap(),
            "--note",
            "the screenshot",
        ],
    );

    let files: Vec<Value> = pending(&w)
        .into_iter()
        .filter(|line| line["t"] == "file")
        .collect();
    assert_eq!(files.len(), 1, "{files:?}");
    let rel = format!("projects/{ID}/evidence/t1/shot.png");
    assert_eq!(files[0]["path"], rel);
    let local = files[0]["local"].as_str().unwrap();
    assert_eq!(std::fs::read(local).unwrap(), [0u8, 159, 146, 150, 255]);

    apply(&w, &[json!({"t": "stored", "path": rel})]);
    assert!(pending(&w).iter().all(|line| line["t"] != "file"));
}

#[test]
fn evidence_in_a_project_that_does_not_share_is_not_uploaded() {
    let w = World::new("share-evidence-off");
    w.project(ID, "walk");
    let shot = w.dir.join("shot.png");
    std::fs::write(&shot, b"png").unwrap();
    walk(
        &w,
        &[
            "evidence",
            "add",
            "--task",
            "t1",
            shot.to_str().unwrap(),
            "--note",
            "the screenshot",
        ],
    );
    assert!(pending(&w).is_empty());
}

#[test]
fn evidence_missing_from_the_replica_is_fetched_through_the_hub() {
    let w = shared("share-evidence-fetch");
    let shot = w.dir.join("shot.png");
    let bytes: Vec<u8> = (0..300_000u32).map(|n| (n % 251) as u8).collect();
    std::fs::write(&shot, &bytes).unwrap();
    let out = walk(
        &w,
        &[
            "evidence",
            "add",
            "--task",
            "t1",
            shot.to_str().unwrap(),
            "--note",
            "the screenshot",
            "--json",
        ],
    );
    let id = serde_json::from_slice::<Value>(&out.stdout).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    // The record arrived from another machine; the bytes did not.
    let stored = w
        .dirs()
        .store()
        .join(format!("projects/{ID}/evidence/t1/shot.png"));
    std::fs::remove_file(&stored).unwrap();

    let socket = w.dir.join("hub.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let served = bytes.clone();
    let asked = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut line)
            .unwrap();
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            served.len()
        );
        (&stream).write_all(head.as_bytes()).unwrap();
        (&stream).write_all(&served).unwrap();
        line
    });
    let out = mem_env(
        &w,
        w.dir.path(),
        &["--project", "walk", "evidence", "cat", &id],
        &[("HUB_LOCAL_SOCKET", socket.to_str().unwrap())],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(out.stdout, bytes);
    assert_eq!(
        asked.join().unwrap().trim(),
        format!("GET /share/file?path=projects/{ID}/evidence/t1/shot.png HTTP/1.1")
    );
    assert_eq!(
        std::fs::read(&stored).unwrap(),
        bytes,
        "kept in the replica"
    );
}
