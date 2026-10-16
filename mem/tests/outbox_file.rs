//! A cloud session's unsent writes travel in `.mem-outbox.json` on the branch
//! it pushes: `mem share export` saves this machine's outbox there, and the
//! next mem run on another machine in that checkout imports it once, keeping
//! each write's id, and stages the file's removal.

mod common;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

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

/// Three logs on the cloud, exported into the cloud's checkout.
fn exported() -> (World, PathBuf, Vec<String>) {
    let cloud = machine("cloud");
    for text in ["outbox probe one", "outbox probe two", "outbox probe three"] {
        walk_in(&cloud, cloud.dir.path(), &["log", text]);
    }
    let repo = checkout(&cloud);
    let out = mem_env(&cloud, &repo, &["share", "export"], &[]);
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
    let out = mem_env(&cloud, &repo, &["share", "export"], &[]);
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
