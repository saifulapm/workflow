//! A cloud session's unsent writes travel in `.mem-outbox.json` on the branch
//! it pushes: `mem share export` saves this machine's outbox there, and the
//! next mem run on another machine in that checkout imports it once, keeping
//! each write's id, and stages the file's removal.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use common::{World, code, mem_env, run_git, stderr, stdout};
use serde_json::{Value, json};

const ID: &str = "01K0SHARE0000000000000000P";
const FILE: &str = ".mem-outbox.json";

/// One machine: its own XDG tree, its own name, the walk project shared.
fn machine(name: &str) -> World {
    let w = World::new(&format!("outbox-{name}"));
    let named = w.dirs().qshell_machine();
    std::fs::create_dir_all(named.parent().unwrap()).unwrap();
    std::fs::write(named, format!("{name}\n")).unwrap();
    w.project(ID, "walk");
    let out = mem_env(
        &w,
        w.dir.path(),
        &["--project", "walk", "project", "set", "share"],
        &[],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    w
}

fn checkout(w: &World) -> PathBuf {
    let repo = w.repo("checkout", None);
    std::fs::write(repo.join("README"), "walk\n").unwrap();
    run_git(&repo, &["add", "README"]);
    commit(&repo, "Start");
    repo
}

fn commit(repo: &Path, subject: &str) {
    run_git(
        repo,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "-m",
            subject,
        ],
    );
}

fn git_out(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "core.hooksPath=/dev/null"])
        .current_dir(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}");
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn walk_in(w: &World, cwd: &Path, args: &[&str]) -> Output {
    let mut all = vec!["--project", "walk"];
    all.extend_from_slice(args);
    let out = mem_env(w, cwd, &all, &[]);
    assert_eq!(code(&out), 0, "mem {args:?}: {}", stderr(&out));
    out
}

fn pending(w: &World) -> Vec<Value> {
    let out = mem_env(w, w.dir.path(), &["share", "pending"], &[]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    stdout(&out)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn ids(writes: &[Value]) -> Vec<String> {
    writes
        .iter()
        .map(|w| w["id"].as_str().unwrap().to_string())
        .collect()
}

fn mem_stdin(w: &World, args: &[&str], input: &str) {
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
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(code(&out), 0, "{}", stderr(&out));
}

fn apply(w: &World, messages: &[Value]) {
    let input: String = messages.iter().map(|m| format!("{m}\n")).collect();
    mem_stdin(w, &["share", "apply"], &input);
}

/// No hub answers in these tests unless one is named: export need not wait
/// for one.
const NO_WAIT: (&str, &str) = ("MEM_SYNC_WAIT_MS", "0");

/// A local hub on its socket that, each time it is kicked, has the store
/// take the oldest waiting writes, `take` of them in all. `body` runs with
/// the socket's path.
fn with_hub<T>(w: &World, take: usize, body: impl FnOnce(&Path) -> T) -> T {
    let socket = w.dir.path().join("hub.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let done = AtomicBool::new(false);
    std::thread::scope(|s| {
        s.spawn(|| {
            let mut taken = 0;
            while !done.load(Ordering::SeqCst) {
                let Ok((stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                };
                stream.set_nonblocking(false).unwrap();
                let mut line = String::new();
                BufReader::new(&stream).read_line(&mut line).unwrap();
                let _ = (&stream).write_all(b"HTTP/1.1 204 No Content\r\n\r\n");
                if !line.contains("/share/kick") {
                    continue;
                }
                for id in ids(&pending(w)).into_iter().take(take - taken) {
                    taken += 1;
                    apply(w, &[json!({"t": "ack", "id": id, "seq": taken})]);
                }
            }
        });
        let out = body(&socket);
        done.store(true, Ordering::SeqCst);
        out
    })
}

/// `mem share export` in `repo` with the hub on `socket`.
fn export_to(w: &World, repo: &Path, socket: &Path) -> String {
    let out = mem_env(
        w,
        repo,
        &["share", "export"],
        &[("HUB_LOCAL_SOCKET", socket.to_str().unwrap())],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    stdout(&out)
}

/// Two logs waiting on the cloud, and its checkout.
fn two_waiting() -> (World, PathBuf) {
    let cloud = machine("cloud");
    for text in ["outbox probe one", "outbox probe two"] {
        walk_in(&cloud, cloud.dir.path(), &["log", text]);
    }
    let repo = checkout(&cloud);
    (cloud, repo)
}

/// Three logs on the cloud, exported into the cloud's checkout.
fn exported() -> (World, PathBuf, Vec<String>) {
    let cloud = machine("cloud");
    for text in ["outbox probe one", "outbox probe two", "outbox probe three"] {
        walk_in(&cloud, cloud.dir.path(), &["log", text]);
    }
    let repo = checkout(&cloud);
    let out = mem_env(&cloud, &repo, &["share", "export"], &[NO_WAIT]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let sent = ids(&pending(&cloud));
    assert_eq!(sent.len(), 3);
    (cloud, repo, sent)
}

#[test]
fn export_saves_the_outbox_in_the_checkout_and_stages_it() {
    let (_cloud, repo, sent) = exported();
    let file: Value =
        serde_json::from_str(&std::fs::read_to_string(repo.join(FILE)).unwrap()).unwrap();
    assert_eq!(file["machine"], "cloud");
    assert_eq!(ids(file["writes"].as_array().unwrap()), sent);
    assert_eq!(
        git_out(&repo, &["diff", "--cached", "--name-only"]),
        ".mem-outbox.json\n"
    );
}

#[test]
fn another_machine_imports_the_file_once_and_stages_its_removal() {
    let (_cloud, repo, sent) = exported();
    commit(&repo, "Save the outbox");
    let mini = machine("mini");

    let log = stdout(&walk_in(&mini, &repo, &["log"]));
    for text in ["outbox probe one", "outbox probe two", "outbox probe three"] {
        assert_eq!(log.matches(text).count(), 1, "{text} in {log}");
    }
    assert_eq!(ids(&pending(&mini)), sent);
    assert!(!repo.join(FILE).exists());
    assert_eq!(
        git_out(&repo, &["diff", "--cached", "--name-status"]),
        "D\t.mem-outbox.json\n"
    );

    // The same file again: nothing doubles.
    run_git(&repo, &["checkout", "HEAD", "--", FILE]);
    walk_in(&mini, &repo, &["log"]);
    assert_eq!(ids(&pending(&mini)), sent);
    assert!(!repo.join(FILE).exists());

    // Once the store has them, a third import sends nothing again.
    let acks: Vec<Value> = sent
        .iter()
        .enumerate()
        .map(|(i, id)| json!({"t": "ack", "id": id, "seq": i + 1}))
        .collect();
    apply(&mini, &acks);
    run_git(&repo, &["checkout", "HEAD", "--", FILE]);
    let log = stdout(&walk_in(&mini, &repo, &["log"]));
    assert!(pending(&mini).is_empty());
    assert_eq!(log.matches("outbox probe one").count(), 1, "{log}");
}

#[test]
fn the_machine_that_wrote_the_file_leaves_it_alone() {
    let (cloud, repo, sent) = exported();
    walk_in(&cloud, &repo, &["log"]);
    assert!(repo.join(FILE).exists());
    assert_eq!(ids(&pending(&cloud)), sent);
    assert_eq!(
        git_out(&repo, &["diff", "--cached", "--name-only"]),
        ".mem-outbox.json\n"
    );
}

#[test]
fn an_imported_page_keeps_its_base_and_lands_nothing_before_the_store_answers() {
    let page = format!("projects/{ID}/wiki/notes.md");
    let first = "01K0OUTBOXREC0000000000001";
    let cloud = machine("cloud");
    let mini = machine("mini");
    for w in [&cloud, &mini] {
        apply(
            w,
            &[json!({"t": "rec", "seq": 1, "id": first, "path": page,
                     "body": "# Notes\n\nfirst\n", "machine": "nuc"})],
        );
    }
    apply(
        &mini,
        &[
            json!({"t": "rec", "seq": 2, "id": "01K0OUTBOXREC0000000000002", "path": page,
                 "body": "# Notes\n\nthe mini's\n", "machine": "nuc"}),
        ],
    );
    mem_stdin(
        &cloud,
        &[
            "--project",
            "walk",
            "wiki",
            "notes",
            "--stdin",
            "--note",
            "edit",
        ],
        "# Notes\n\nthe cloud's\n",
    );
    let repo = checkout(&cloud);
    let out = mem_env(&cloud, &repo, &["share", "export"], &[NO_WAIT]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));

    let wiki = stdout(&walk_in(&mini, &repo, &["wiki", "notes"]));
    assert_eq!(wiki, "# Notes\n\nthe mini's\n");
    let write = pending(&mini)
        .into_iter()
        .find(|w| w["path"] == page)
        .expect("the cloud's page write waits on the mini");
    assert_eq!(write["base"], first);
    assert_eq!(write["body"], "# Notes\n\nthe cloud's\n");
}

/// Puts a cloud's outbox file holding `writes` in a checkout of `w`'s and
/// runs mem there, which imports it.
fn import(w: &World, writes: Value) {
    let repo = checkout(w);
    let file = json!({"machine": "cloud", "writes": writes});
    std::fs::write(repo.join(FILE), file.to_string()).unwrap();
    walk_in(w, &repo, &["projects"]);
    assert!(!repo.join(FILE).exists());
}

fn notes(w: &World) -> String {
    stdout(&walk_in(w, w.dir.path(), &["wiki", "notes"]))
}

const PAGE_X: &str = "01K0OUTBOXX000000000000001";

#[test]
fn a_late_ack_of_an_imported_page_never_lands_over_a_newer_one() {
    let page = format!("projects/{ID}/wiki/notes.md");
    let mini = machine("mini");
    apply(
        &mini,
        &[
            json!({"t": "rec", "seq": 7, "id": "01K0OUTBOXY000000000000001", "path": page,
                 "body": "# Notes\n\nY\n", "machine": "mini"}),
        ],
    );
    import(
        &mini,
        json!([{"id": PAGE_X, "path": page, "base": null, "body": "# Notes\n\nX\n"}]),
    );
    apply(&mini, &[json!({"t": "ack", "id": PAGE_X, "seq": 5})]);
    assert_eq!(notes(&mini), "# Notes\n\nY\n");
    assert!(pending(&mini).is_empty());
}

#[test]
fn a_fresh_ack_of_an_imported_page_lands_it() {
    let page = format!("projects/{ID}/wiki/notes.md");
    let mini = machine("mini");
    import(
        &mini,
        json!([{"id": PAGE_X, "path": page, "base": null, "body": "# Notes\n\nX\n"}]),
    );
    apply(&mini, &[json!({"t": "ack", "id": PAGE_X, "seq": 3})]);
    assert_eq!(notes(&mini), "# Notes\n\nX\n");
}

#[test]
fn an_imported_page_replayed_before_its_ack_lands_it() {
    // The store already held X: the replay after hello brings X while it
    // still waits here, and the re-ack carries the same seq.
    let page = format!("projects/{ID}/wiki/notes.md");
    let mini = machine("mini");
    import(
        &mini,
        json!([{"id": PAGE_X, "path": page, "base": null, "body": "# Notes\n\nX\n"}]),
    );
    apply(
        &mini,
        &[
            json!({"t": "rec", "seq": 5, "id": PAGE_X, "path": page,
                   "body": "# Notes\n\nX\n", "machine": "cloud"}),
            json!({"t": "ack", "id": PAGE_X, "seq": 5}),
        ],
    );
    assert_eq!(notes(&mini), "# Notes\n\nX\n");
}

#[test]
fn an_imported_item_never_replaces_one_the_replica_holds() {
    let mini = machine("mini");
    let mut item = common::item(mem::item::Kind::Log, "outbox item", "the mini's\n");
    item.meta.id = "01K0OUTBOX1TEM000000000001".to_string();
    item.meta.project = Some("walk".to_string());
    let path = format!("projects/{ID}/items/{}.md", item.meta.id);
    let held = String::from_utf8(item.to_bytes().unwrap()).unwrap();
    apply(
        &mini,
        &[
            json!({"t": "rec", "seq": 7, "id": "01K0OUTBOXW000000000000001", "path": path,
                 "body": held, "machine": "mini"}),
        ],
    );
    item.body = b"the cloud's older\n".to_vec();
    let older = String::from_utf8(item.to_bytes().unwrap()).unwrap();
    import(
        &mini,
        json!([{"id": "01K0OUTBOXW000000000000000", "path": path, "base": null, "body": older}]),
    );
    let file = mini.dirs().store().join(&path);
    assert_eq!(std::fs::read_to_string(file).unwrap(), held);
}

/// A branch's file speaks only for projects this machine knows, or one it
/// brings with its own project.toml: a forged write for any other project
/// imports nothing, and the file stays for a person to look at.
#[test]
fn a_write_for_a_project_this_machine_does_not_know_imports_nothing() {
    let mini = machine("mini");
    let repo = checkout(&mini);
    let stranger = "projects/01K0NKN0WNPR0JECT000000000/items/01K0OUTBOX1TEM000000000009.md";
    let file = json!({"machine": "cloud", "writes": [
        {"id": "01K0OUTBOXW000000000000009", "path": stranger, "base": null, "body": "x\n"},
    ]});
    std::fs::write(repo.join(FILE), file.to_string()).unwrap();
    let out = walk_in(&mini, &repo, &["projects"]);
    assert!(stderr(&out).contains("left in place"), "{}", stderr(&out));
    assert!(repo.join(FILE).exists());
    assert!(!mini.dirs().store().join(stranger).exists());
    assert!(pending(&mini).is_empty());
}

/// A project made in the cloud travels with its project.toml, so its
/// writes import.
#[test]
fn a_project_the_file_brings_imports_with_it() {
    let mini = machine("mini");
    let new = "01K0NEWPR0JECT000000000000";
    let toml = format!(
        "created = \"2026-10-16T00:00:00Z\"\nid = \"{new}\"\nname = \"fresh\"\nshare = \"true\"\n"
    );
    import(
        &mini,
        json!([
            {"id": "01K0OUTBOXW000000000000010", "path": format!("projects/{new}/project.toml"), "base": null, "body": toml},
            {"id": "01K0OUTBOXW000000000000011", "path": format!("projects/{new}/wiki/notes.md"), "base": null, "body": "# Notes\n"},
        ]),
    );
    assert_eq!(pending(&mini).len(), 2);
}

/// Even in a known project, a file speaks only for mem's own records.
#[test]
fn a_path_that_is_no_record_imports_nothing() {
    for rest in [
        "items/notulid.md",
        "stray.txt",
        "wiki/a/b.md",
        "items/.hidden.md",
    ] {
        let mini = machine("mini");
        let repo = checkout(&mini);
        let path = format!("projects/{ID}/{rest}");
        let file = json!({"machine": "cloud", "writes": [
            {"id": "01K0OUTBOXW000000000000020", "path": path, "base": null, "body": "x\n"},
        ]});
        std::fs::write(repo.join(FILE), file.to_string()).unwrap();
        let out = walk_in(&mini, &repo, &["projects"]);
        assert!(
            stderr(&out).contains("left in place"),
            "{rest}: {}",
            stderr(&out)
        );
        assert!(repo.join(FILE).exists(), "{rest}");
        assert!(pending(&mini).is_empty(), "{rest}");
    }
}

/// A thread exported right after each write, before the hub had sent it, so
/// threads put 94 outbox commits on four projects' mains in a day. Export
/// now has the hub send first and saves only what the store has not taken.
#[test]
fn export_saves_nothing_once_the_hub_has_sent_every_write() {
    let (cloud, repo) = two_waiting();
    let said = with_hub(&cloud, 2, |socket| export_to(&cloud, &repo, socket));
    assert_eq!(said, "saved 0 writes to .mem-outbox.json\n");
    assert_eq!(git_out(&repo, &["status", "--short"]), "");
    assert!(pending(&cloud).is_empty());
}

#[test]
fn export_saves_the_write_the_hub_did_not_send() {
    let (cloud, repo) = two_waiting();
    let waiting = ids(&pending(&cloud));
    let said = with_hub(&cloud, 1, |socket| {
        let out = mem_env(
            &cloud,
            &repo,
            &["share", "export"],
            &[
                ("HUB_LOCAL_SOCKET", socket.to_str().unwrap()),
                ("MEM_SYNC_WAIT_MS", "500"),
            ],
        );
        assert_eq!(code(&out), 0, "{}", stderr(&out));
        stdout(&out)
    });
    assert_eq!(said, "saved 1 write to .mem-outbox.json\n");
    let file: Value =
        serde_json::from_str(&std::fs::read_to_string(repo.join(FILE)).unwrap()).unwrap();
    assert_eq!(ids(file["writes"].as_array().unwrap()), waiting[1..]);
}

/// With no hub to send them, the writes go in the file as before, and the
/// wait ends at MEM_SYNC_WAIT_MS.
#[test]
fn with_no_hub_export_saves_every_write_once_the_wait_ends() {
    let (cloud, repo) = two_waiting();
    let waiting = ids(&pending(&cloud));
    let started = Instant::now();
    let out = mem_env(
        &cloud,
        &repo,
        &["share", "export"],
        &[("MEM_SYNC_WAIT_MS", "200")],
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&out), "saved 2 writes to .mem-outbox.json\n");
    let file: Value =
        serde_json::from_str(&std::fs::read_to_string(repo.join(FILE)).unwrap()).unwrap();
    assert_eq!(ids(file["writes"].as_array().unwrap()), waiting);
    assert_eq!(
        git_out(&repo, &["diff", "--cached", "--name-only"]),
        ".mem-outbox.json\n"
    );
}

/// A machine that never shared through a hub has nothing to send, so export
/// does not wait the default ten seconds.
#[test]
fn a_machine_that_never_shared_does_not_wait() {
    let w = World::new("outbox-unshared");
    w.project(ID, "walk");
    walk_in(&w, w.dir.path(), &["log", "not shared"]);
    let repo = checkout(&w);
    let started = Instant::now();
    let out = mem_env(&w, &repo, &["share", "export"], &[]);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&out), "saved 0 writes to .mem-outbox.json\n");
}

/// The store unreachable for one export and back for the next: the second
/// takes the first one's file off the branch.
#[test]
fn an_export_once_the_store_is_back_removes_the_file_an_earlier_one_saved() {
    let (cloud, repo) = two_waiting();
    let out = mem_env(&cloud, &repo, &["share", "export"], &[NO_WAIT]);
    assert_eq!(stdout(&out), "saved 2 writes to .mem-outbox.json\n");
    commit(&repo, "Carry the writes the store has not taken yet");
    let said = with_hub(&cloud, 2, |socket| export_to(&cloud, &repo, socket));
    assert_eq!(said, "saved 0 writes to .mem-outbox.json\n");
    assert!(!repo.join(FILE).exists());
    assert_eq!(
        git_out(&repo, &["diff", "--cached", "--name-status"]),
        "D\t.mem-outbox.json\n"
    );
}
