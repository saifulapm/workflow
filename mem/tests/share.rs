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
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        // Read the rest of the request: closing a socket with unread bytes
        // resets it, and mem would lose the reply.
        loop {
            let mut header = String::new();
            if reader.read_line(&mut header).unwrap() == 0 || header == "\r\n" {
                break;
            }
        }
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

#[test]
fn a_streamed_write_for_a_project_this_replica_does_not_share_is_not_landed() {
    let w = shared("share-other-project");
    w.project("01K0SHARE0THER00000000000P", "other");
    let other = "projects/01K0SHARE0THER00000000000P/wiki/notes.md";
    let unknown = "projects/01K0SHARE0NKN0WN0000000000P/wiki/notes.md";
    apply(
        &w,
        &[
            json!({"t": "rec", "seq": 1, "id": "01K0SHAREREC00000000000011", "path": other,
                   "body": "# Notes\n", "machine": "nuc"}),
            json!({"t": "rec", "seq": 2, "id": "01K0SHAREREC00000000000012", "path": unknown,
                   "body": "# Notes\n", "machine": "nuc"}),
        ],
    );
    let store = w.dirs().store();
    assert!(!store.join(other).exists());
    assert!(!store.join("projects/01K0SHARE0NKN0WN0000000000P").exists());
    assert_eq!(hello(&w)["since"], 2);
}

#[test]
fn a_write_from_elsewhere_skipped_behind_a_lost_ack_lands_when_the_ack_comes() {
    let w = shared("share-lost-ack");
    wiki_set(&w, "lessons", "# Lessons\n\nmine\n");
    let mine = pending(&w)
        .into_iter()
        .find(|write| write["path"] == page_path("lessons"))
        .unwrap();
    // On reconnect the replay brings the nuc's later write to the same page
    // while mine still waits; then the resend of mine is acked with its
    // first, older seq.
    apply(
        &w,
        &[
            json!({"t": "rec", "seq": 10, "id": mine["id"], "path": page_path("lessons"),
                   "body": "# Lessons\n\nmine\n", "machine": "mini"}),
            json!({"t": "rec", "seq": 11, "id": "01K0SHAREREC00000000000021", "path": page_path("lessons"),
                   "body": "# Lessons\n\ntheirs, later\n", "machine": "nuc"}),
            json!({"t": "ready", "seq": 11}),
            json!({"t": "ack", "id": mine["id"], "seq": 10}),
        ],
    );
    assert_eq!(wiki(&w, "lessons"), "# Lessons\n\ntheirs, later\n");
    wiki_set(&w, "lessons", "# Lessons\n\nnext\n");
    let next = pending(&w)
        .into_iter()
        .find(|write| write["path"] == page_path("lessons"))
        .unwrap();
    assert_eq!(next["base"], "01K0SHAREREC00000000000021");
}

#[test]
fn a_write_from_elsewhere_older_than_mine_does_not_replace_mine_on_the_ack() {
    let w = shared("share-older");
    walk(&w, &["log", "mine"]);
    let mine = pending(&w).remove(0);
    let path = mine["path"].as_str().unwrap().to_string();
    apply(
        &w,
        &[
            json!({"t": "rec", "seq": 9, "id": "01K0SHAREREC00000000000022", "path": path,
                   "body": "older", "machine": "nuc"}),
            json!({"t": "ack", "id": mine["id"], "seq": 10}),
        ],
    );
    let file = w.dirs().store().join(&path);
    assert_eq!(
        std::fs::read_to_string(file).unwrap(),
        mine["body"].as_str().unwrap()
    );
}

#[test]
fn a_streamed_write_that_cannot_land_is_passed_over_and_the_rest_land() {
    let w = shared("share-poison");
    apply(
        &w,
        &[
            json!({"t": "rec", "seq": 12, "id": "01K0SHAREREC00000000000023",
                   "path": format!("projects/{ID}/items"), "body": "x", "machine": "nuc"}),
            json!({"t": "rec", "seq": 13, "id": "01K0SHAREREC00000000000024", "path": page_path("after"),
                   "body": "# After\n", "machine": "nuc"}),
        ],
    );
    assert_eq!(wiki(&w, "after"), "# After\n");
    assert_eq!(hello(&w)["since"], 13);
}

#[test]
fn applied_with_all_a_project_this_replica_has_never_seen_lands() {
    let w = World::new("share-all");
    let project = "projects/01K0SHARE0NKN0WN0000000000P";
    let toml = "created = \"2026-10-16T00:00:00Z\"\nid = \"01K0SHARE0NKN0WN0000000000P\"\nname = \"far\"\nshare = \"true\"\n";
    let input: String = [
        json!({"t": "rec", "seq": 1, "id": "01K0SHAREREC00000000000031",
               "path": format!("{project}/project.toml"), "body": toml, "machine": "mini"}),
        json!({"t": "rec", "seq": 2, "id": "01K0SHAREREC00000000000032",
               "path": format!("{project}/wiki/notes.md"), "body": "# Notes\n", "machine": "mini"}),
        json!({"t": "ready", "seq": 2}),
    ]
    .iter()
    .map(|m| format!("{m}\n"))
    .collect();
    let out = mem_in(&w, &["share", "apply", "--all"], &input);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let notes = mem(&w, w.dir.path(), &["--project", "far", "wiki", "notes"]);
    assert_eq!(stdout(&notes), "# Notes\n", "{}", stderr(&notes));
    assert_eq!(hello(&w)["since"], 2);
}

#[test]
fn sharing_a_project_late_asks_the_store_for_everything_again() {
    let w = World::new("share-late");
    w.project(ID, "walk");
    apply(
        &w,
        &[
            json!({"t": "rec", "seq": 5, "id": "01K0SHAREREC00000000000025", "path": page_path("early"),
                 "body": "# Early\n", "machine": "nuc"}),
        ],
    );
    assert_eq!(hello(&w)["since"], 5);
    walk(&w, &["project", "set", "share"]);

    let lines = pending(&w);
    assert_eq!(lines[0]["t"], "hello", "{lines:?}");
    assert_eq!(lines[0]["since"], 0);
    apply(
        &w,
        &[
            json!({"t": "rec", "seq": 5, "id": "01K0SHAREREC00000000000025", "path": page_path("early"),
                   "body": "# Early\n", "machine": "nuc"}),
            json!({"t": "ready", "seq": 5}),
        ],
    );
    assert_eq!(wiki(&w, "early"), "# Early\n");
    assert!(pending(&w).iter().all(|line| line["t"] != "hello"));
}

#[test]
fn a_write_the_replica_refuses_is_already_in_the_outbox() {
    use std::os::unix::fs::PermissionsExt;
    let w = shared("share-outbox-first");
    let items = w.dirs().store().join(format!("projects/{ID}/items"));
    std::fs::set_permissions(&items, std::fs::Permissions::from_mode(0o555)).unwrap();
    // The replica cannot take the file, as if mem died before the rename.
    let _ = mem(
        &w,
        w.dir.path(),
        &["--project", "walk", "log", "still sent"],
    );
    std::fs::set_permissions(&items, std::fs::Permissions::from_mode(0o755)).unwrap();
    let writes = pending(&w);
    assert_eq!(writes.len(), 1, "{writes:?}");
    assert!(writes[0]["body"].as_str().unwrap().contains("still sent"));

    apply(&w, &[json!({"t": "ack", "id": writes[0]["id"], "seq": 1})]);
    let file = w.dirs().store().join(writes[0]["path"].as_str().unwrap());
    assert!(
        std::fs::read_to_string(file)
            .unwrap()
            .contains("still sent")
    );
}

#[test]
fn a_raw_file_missing_from_the_replica_is_fetched_through_the_hub() {
    let w = shared("share-raw-fetch");
    let src = w.dir.join("big.bin");
    let bytes: Vec<u8> = (0..200_000u32).map(|n| (n % 241) as u8).collect();
    std::fs::write(&src, &bytes).unwrap();
    let out = walk(&w, &["raw", "add", src.to_str().unwrap(), "--json"]);
    let id = serde_json::from_slice::<Value>(&out.stdout).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let stored = w.dirs().store().join(format!("projects/{ID}/raw/big.bin"));
    std::fs::remove_file(&stored).unwrap();

    let socket = w.dir.join("hub.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let served = bytes.clone();
    std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        loop {
            let mut header = String::new();
            if reader.read_line(&mut header).unwrap() == 0 || header == "\r\n" {
                break;
            }
        }
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            served.len()
        );
        (&stream).write_all(head.as_bytes()).unwrap();
        (&stream).write_all(&served).unwrap();
    });
    let out = mem_env(
        &w,
        w.dir.path(),
        &["--project", "walk", "raw", "cat", &id],
        &[("HUB_LOCAL_SOCKET", socket.to_str().unwrap())],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(out.stdout, bytes);
}

fn switched(tag: &str) -> World {
    let w = World::new(tag);
    w.project(ID, "walk");
    let out = mem(&w, w.dir.path(), &["share", "switch"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    w
}

#[test]
fn a_switched_machine_shares_a_project_that_never_said_share() {
    let w = switched("share-switched-project");
    walk(&w, &["log", "after the switch"]);
    let writes = pending(&w);
    assert_eq!(writes.len(), 1, "{writes:?}");
    assert!(
        writes[0]["path"]
            .as_str()
            .unwrap()
            .starts_with(&format!("projects/{ID}/items/"))
    );
}

#[test]
fn a_switched_machine_shares_a_global_item() {
    let w = switched("share-switched-global");
    let out = mem(&w, w.dir.path(), &["save", "a standing rule", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let item: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        item["path"].as_str().unwrap().contains("/global/items/"),
        "{item}"
    );
    let writes = pending(&w);
    assert_eq!(writes.len(), 1, "{writes:?}");
    assert!(
        writes[0]["path"]
            .as_str()
            .unwrap()
            .starts_with("global/items/"),
        "{writes:?}"
    );
}

#[test]
fn a_machine_that_has_not_switched_keeps_global_items_to_itself() {
    let w = shared("share-global-unswitched");
    let out = mem(&w, w.dir.path(), &["save", "a standing rule"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(pending(&w).is_empty());
}

#[test]
fn a_streamed_global_item_lands_in_a_switched_replica() {
    let w = switched("share-global-lands");
    let rel = "global/items/01K0SHAREGLOBAL00000000001.md";
    apply(
        &w,
        &[
            json!({"t": "rec", "seq": 1, "id": "01K0SHAREREC0000000000000G",
                 "path": rel, "body": "+++\nkind = \"fact\"\n+++\n\nrule\n", "machine": "nuc"}),
        ],
    );
    assert!(w.dirs().store().join(rel).exists());
    assert_eq!(hello(&w)["since"], 1);
}

#[test]
fn a_streamed_global_item_is_not_landed_where_the_machine_has_not_switched() {
    let w = shared("share-global-not-landed");
    let rel = "global/items/01K0SHAREGLOBAL00000000002.md";
    apply(
        &w,
        &[
            json!({"t": "rec", "seq": 1, "id": "01K0SHAREREC0000000000000H",
                 "path": rel, "body": "x", "machine": "nuc"}),
        ],
    );
    assert!(!w.dirs().store().join(rel).exists());
}

#[test]
fn switching_asks_the_store_for_every_write_again() {
    let w = shared("share-switch-resync");
    apply(&w, &[json!({"t": "ready", "seq": 7})]);
    assert_eq!(hello(&w)["since"], 7);
    let out = mem(&w, w.dir.path(), &["share", "switch"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(hello(&w)["since"], 0);
}

/// The import's plan: what `mem share seed` prints for the store's replay
/// fed on stdin, one JSON per line.
fn seed(w: &World, store: &[Value]) -> Vec<Value> {
    let machine = w.dirs().qshell_machine();
    if !machine.exists() {
        std::fs::create_dir_all(machine.parent().unwrap()).unwrap();
        std::fs::write(&machine, "nuc\n").unwrap();
    }
    let input: String = store.iter().map(|m| format!("{m}\n")).collect();
    let out = mem_in(w, &["share", "seed"], &input);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    stdout(&out)
        .lines()
        .map(|line| serde_json::from_str(line).expect("one JSON per line"))
        .collect()
}

fn put(w: &World, rel: &str, body: &str) {
    let file = w.dirs().store().join(rel);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, body).unwrap();
}

fn rec(seq: u64, id: &str, path: &str, body: Value) -> Value {
    json!({"t": "rec", "seq": seq, "id": id, "path": path, "body": body, "machine": "nuc"})
}

fn by_path<'a>(plan: &'a [Value], path: &str) -> Option<&'a Value> {
    plan.iter().find(|m| m["path"] == path)
}

#[test]
fn the_import_sends_what_the_store_lacks_or_holds_otherwise() {
    let w = World::new("seed-plan");
    w.project(ID, "walk");
    let same = format!("projects/{ID}/items/01K0SEEDSAME0000000000000A.md");
    let page = format!("projects/{ID}/wiki/lessons.md");
    let rule = "global/items/01K0SEEDRULE0000000000000A.md";
    put(&w, &same, "same body");
    put(&w, &page, "newer lessons");
    put(&w, rule, "a rule");
    let plan = seed(
        &w,
        &[
            rec(1, "01K0SEEDREC00000000000000A", &same, json!("same body")),
            rec(2, "01K0SEEDREC00000000000000B", &page, json!("old lessons")),
            rec(3, "01K0SEEDREC00000000000000C", &page, json!("lessons v2")),
            json!({"t": "ready", "seq": 3, "rows": 0}),
        ],
    );
    assert!(by_path(&plan, &same).is_none(), "{plan:?}");
    let sent = by_path(&plan, &page).expect("the page differs");
    assert_eq!(sent["t"], "write");
    assert_eq!(sent["base"], "01K0SEEDREC00000000000000C");
    assert_eq!(sent["body"], "newer lessons");
    assert_eq!(sent["import"], true);
    assert_eq!(sent["id"].as_str().unwrap().len(), 26);
    let sent = by_path(&plan, rule).expect("the store lacks the rule");
    assert_eq!(sent["base"], Value::Null);
    assert_eq!(sent["body"], "a rule");
    let toml = format!("projects/{ID}/project.toml");
    assert!(by_path(&plan, &toml).is_some(), "{plan:?}");
}

#[test]
fn the_import_removes_from_the_store_what_this_store_no_longer_holds() {
    let w = World::new("seed-removal");
    w.project(ID, "walk");
    let plan_md = format!("projects/{ID}/plan.md");
    let elsewhere = "projects/01K0SEEDOTHERPROJECT000000/plan.md";
    let plan = seed(
        &w,
        &[
            rec(
                1,
                "01K0SEEDREC00000000000000D",
                &plan_md,
                json!("# plan: m1"),
            ),
            rec(
                2,
                "01K0SEEDREC00000000000000E",
                elsewhere,
                json!("# plan: x"),
            ),
            rec(
                3,
                "01K0SEEDREC00000000000000F",
                &format!("projects/{ID}/roadmap.md"),
                Value::Null,
            ),
        ],
    );
    let removal = by_path(&plan, &plan_md).expect("the plan was cleared here");
    assert_eq!(removal["body"], Value::Null);
    assert_eq!(removal["base"], "01K0SEEDREC00000000000000D");
    assert!(by_path(&plan, elsewhere).is_none(), "{plan:?}");
    assert!(by_path(&plan, &format!("projects/{ID}/roadmap.md")).is_none());
}

#[test]
fn the_import_never_sends_conflict_copies_temps_or_the_version() {
    let w = World::new("seed-skips");
    w.project(ID, "walk");
    put(&w, &format!("projects/{ID}/plan.md.conflict1"), "x");
    put(&w, &format!("projects/{ID}/items/.tmp-123"), "x");
    put(&w, "VERSION", "3");
    let plan = seed(&w, &[]);
    let paths: Vec<&str> = plan.iter().map(|m| m["path"].as_str().unwrap()).collect();
    assert_eq!(paths, [format!("projects/{ID}/project.toml").as_str()]);
}

#[test]
fn the_import_uploads_every_file_once() {
    let w = World::new("seed-files");
    w.project(ID, "walk");
    let fresh = format!("projects/{ID}/evidence/m1/shot.png");
    let old = format!("projects/{ID}/raw/old.bin");
    put(&w, &fresh, "png");
    put(&w, &old, "bin");
    let month = Duration::from_secs(86_400 * 100);
    std::fs::File::options()
        .write(true)
        .open(w.dirs().store().join(&old))
        .unwrap()
        .set_modified(std::time::SystemTime::now() - month)
        .unwrap();
    let plan = seed(&w, &[]);
    let file = by_path(&plan, &fresh).expect("the fresh file goes");
    assert_eq!(file["t"], "file");
    assert_eq!(
        file["local"],
        w.dirs().store().join(&fresh).to_str().unwrap()
    );
    assert_eq!(
        by_path(&plan, &old).expect("the old file goes too")["t"],
        "file"
    );

    let out = mem_in(&w, &["share", "seeded"], &format!("{fresh}\n"));
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let again = seed(&w, &[]);
    assert!(by_path(&again, &fresh).is_none());
    assert!(by_path(&again, &old).is_some());
}

#[test]
fn the_import_leaves_the_outbox_and_the_replicas_heads_alone() {
    let w = World::new("seed-alone");
    w.project(ID, "walk");
    put(&w, &format!("projects/{ID}/wiki/lessons.md"), "lessons");
    seed(&w, &[json!({"t": "ready", "seq": 9, "rows": 0})]);
    assert!(pending(&w).is_empty());
    assert_eq!(hello(&w)["since"], 0);
}

#[test]
fn doctor_on_a_switched_machine_does_not_ask_for_the_memory_unit() {
    let w = switched("share-switched-doctor");
    let out = mem(&w, w.dir.path(), &["doctor"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let text = stdout(&out);
    assert!(!text.lines().any(|l| l.starts_with("sync ")), "{text}");
    let unswitched = shared("share-unswitched-doctor");
    let text = stdout(&mem(&unswitched, unswitched.dir.path(), &["doctor"]));
    assert!(text.lines().any(|l| l.starts_with("sync ")), "{text}");
}

fn rec_by(seq: u64, id: &str, path: &str, body: Value, machine: &str) -> Value {
    json!({"t": "rec", "seq": seq, "id": id, "path": path, "body": body, "machine": machine})
}

#[test]
fn the_import_leaves_alone_what_a_switched_machine_wrote_and_keeps_its_own_copy() {
    let w = World::new("seed-switched-wrote");
    w.project(ID, "walk");
    let page = format!("projects/{ID}/wiki/lessons.md");
    let item = format!("projects/{ID}/items/01K0SEEDITEM00000000000000.md");
    put(&w, &page, "lessons as the VPS has them");
    put(&w, &item, "item, archived on the VPS");
    let plan = seed(
        &w,
        &[
            rec_by(
                1,
                "01K0SEEDREC00000000000001A",
                &page,
                json!("lessons the mini wrote"),
                "mini",
            ),
            rec_by(
                2,
                "01K0SEEDREC00000000000001B",
                &item,
                json!("item"),
                "mini",
            ),
        ],
    );
    assert!(
        !plan.iter().any(|m| m["t"] == "write" && m["path"] == page),
        "{plan:?}"
    );
    assert!(by_path(&plan, &item).is_none(), "{plan:?}");
    let kept = by_path(&plan, &page).expect("the plan says the copy was kept");
    assert_eq!(kept["t"], "kept");
    let copy = w.dirs().store().join(format!("{page}.conflict-import"));
    assert_eq!(
        std::fs::read_to_string(&copy).unwrap(),
        "lessons as the VPS has them"
    );
    assert!(
        !w.dirs()
            .store()
            .join(format!("{item}.conflict-import"))
            .exists()
    );
}

#[test]
fn the_import_never_removes_what_a_switched_machine_wrote() {
    let w = World::new("seed-switched-removal");
    w.project(ID, "walk");
    let item = format!("projects/{ID}/items/01K0SEEDMINI00000000000000.md");
    let plan = seed(
        &w,
        &[rec_by(
            1,
            "01K0SEEDREC00000000000001C",
            &item,
            json!("the mini's log"),
            "mini",
        )],
    );
    assert!(by_path(&plan, &item).is_none(), "{plan:?}");
}
