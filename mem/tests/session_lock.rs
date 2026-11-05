//! Two hooks of one session, one in each of two projects, run at the same
//! moment. The session file must keep both projects' briefs.

mod common;

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::{World, code, mem, stderr, stdout};

const HOOK: [&str; 4] = ["context", "--brief", "--hook-json", "--session-id"];

/// Two registered projects, each in its own checkout. Returns the checkouts
/// and the projects' ids.
fn two_projects(w: &World) -> ([std::path::PathBuf; 2], [String; 2]) {
    let a = w.repo("alpha", Some("git@github.com:me/alpha.git"));
    let b = w.repo("beta", Some("git@github.com:me/beta.git"));
    for (repo, text) in [(&a, "alpha fact"), (&b, "beta fact")] {
        let out = mem(w, repo, &["save", text]);
        assert_eq!(code(&out), 0, "{}", stderr(&out));
    }
    let registry = mem::project::Registry::load(&w.store());
    let id = |name: &str| {
        registry
            .projects
            .iter()
            .find(|p| p.name == name)
            .unwrap_or_else(|| panic!("{name} is registered"))
            .id
            .clone()
    };
    ([a, b], [id("alpha"), id("beta")])
}

fn spawn_hook(w: &World, cwd: &Path, session: &str) -> std::process::Child {
    let dirs = w.dirs();
    Command::new(env!("CARGO_BIN_EXE_mem"))
        .current_dir(cwd)
        .args(HOOK)
        .arg(session)
        .env("XDG_DATA_HOME", &dirs.data)
        .env("XDG_CACHE_HOME", &dirs.cache)
        .env("XDG_STATE_HOME", &dirs.state)
        .env("XDG_CONFIG_HOME", &dirs.config)
        .env("PI_CODING_AGENT_DIR", w.pi_agent_dir())
        .env("MEM_SYNC_CMD", "true")
        .env_remove("MEM_PROJECT")
        .env_remove("HUB_LOCAL_SOCKET")
        .env_remove("CARGO_TARGET_DIR")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn mem")
}

#[test]
fn racing_hooks_in_two_projects_keep_both_briefs() {
    let w = World::new("session-race");
    let ([a, b], [id_a, id_b]) = two_projects(&w);
    let sessions = w.dirs().sessions_dir();

    let mut lost = Vec::new();
    let mut last = String::new();
    for round in 0..50 {
        let session = format!("race-{round}");
        let mut first = spawn_hook(&w, &a, &session);
        let mut second = spawn_hook(&w, &b, &session);
        first.wait().unwrap();
        second.wait().unwrap();
        let briefs = mem::session::read(&sessions, &session).briefs;
        if !(briefs.contains_key(&id_a) && briefs.contains_key(&id_b)) {
            lost.push((round, briefs.keys().cloned().collect::<Vec<_>>()));
        }
        last = session;
    }
    assert!(
        lost.is_empty(),
        "{} of 50 rounds lost a brief: {lost:?}",
        lost.len()
    );

    // Both briefs are on record, so the next batch has nothing new to say.
    let out = mem(&w, &a, &[HOOK[0], HOOK[1], HOOK[2], HOOK[3], &last]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&out), "");
}

#[test]
fn a_lock_left_by_a_dead_process_does_not_stop_a_hook() {
    let w = World::new("session-dead-lock");
    let ([a, _], [id_a, _]) = two_projects(&w);
    let sessions = w.dirs().sessions_dir();
    std::fs::create_dir_all(&sessions).unwrap();
    let lock = sessions.join("dead-s.lock");

    // A holder takes the flock, then is killed without releasing it.
    let mut holder = Command::new("sh")
        .arg("-c")
        .arg(r#"exec 9>"$1"; flock 9; echo held; exec sleep 600"#)
        .arg("sh")
        .arg(&lock)
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn lock holder");
    let mut held = String::new();
    holder
        .stdout
        .take()
        .unwrap()
        .take(5)
        .read_to_string(&mut held)
        .unwrap();
    assert_eq!(held, "held\n");
    holder.kill().unwrap();
    holder.wait().unwrap();

    let started = Instant::now();
    let mut child = spawn_hook(&w, &a, "dead-s");
    let status = child.wait().unwrap();
    assert!(status.success());
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    assert!(
        mem::session::read(&sessions, "dead-s")
            .briefs
            .contains_key(&id_a)
    );
}
