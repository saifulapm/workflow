//! The amx worker backend (spec §12b; the design is `mem wiki amx-backend`).
//!
//! Same seam, different substrate: a worker is an amx agent in a tmux pane
//! rather than a `claude --bg` session, and nothing above [`WorkerBackend`]
//! knows which it got. amx answers all four questions off one verb --
//! `amx status <id> --json` -- so liveness, the record and the ending are one
//! parse of one document rather than a listing, a transcript and a pidfile.
//!
//! `WORKFLOW_AMX` names the binary, defaulting to `amx` on PATH. It is this
//! backend's test seam, the way `WORKFLOW_WORKER_CMD` is the claude backend's;
//! the two mean nothing to each other.

use std::path::Path;
use std::process::{Command, Stdio};

use crate::backend::{Dispatch, Handle, Outcome, WorkerBackend};
use crate::sys;

/// The phases that mean nothing more is coming from this agent.
///
/// `waiting` is one of them: an agent stopped on a question has ended its
/// turn, and the status file protocol is what answers it -- the worker writes
/// `blocked` and asks through `mem ask`, exactly as under the claude backend.
///
/// The live phases are the ones not here: `starting`, `working`, and
/// `unknown`, which is amx saying it cannot read the pane rather than that the
/// pane is finished. Treating a screen amx cannot account for as an ending
/// would collect a worker mid-turn; left alive, the stall deadline decides it.
const ENDINGS: [&str; 5] = ["waiting", "idle", "done", "failed", "stopped"];

/// The phases that end a turn without an error. Everything else that ends --
/// `waiting` on a question, `failed`, `stopped`, an unreadable screen -- is
/// not a clean ending. What the work was worth is still the status file's and
/// the merge gate's to say, exactly as for claude.
const CLEAN: [&str; 2] = ["idle", "done"];

const BASE36: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// Prefix on every agent this backend starts, so a person reading `amx ls`
/// can tell an orchestrated worker from one they started themselves.
const PREFIX: &str = "wf";

pub struct AmxBackend;

fn amx_bin() -> String {
    match std::env::var("WORKFLOW_AMX") {
        Ok(v) if !v.is_empty() => v,
        _ => "amx".to_string(),
    }
}

/// What one `amx` call said on stdout, and whether it exited 0. A binary that
/// cannot be run at all reads as a failed call with nothing to say, which is
/// the same answer as amx refusing -- and both mean "no record here".
fn amx(args: &[&str]) -> (String, bool) {
    let Ok(out) = Command::new(amx_bin())
        .args(args)
        .stderr(Stdio::null())
        .output()
    else {
        return (String::new(), false);
    };
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.success(),
    )
}

/// The two fields of `amx status --json` this backend reads.
#[derive(Debug, Clone, Default, PartialEq)]
struct Status {
    state: String,
    last_event: i64,
}

/// The pure half of [`status`], so the parse is testable without an amx.
fn status_in(json: &str) -> Option<Status> {
    let Ok(serde_json::Value::Object(v)) = serde_json::from_str::<serde_json::Value>(json) else {
        return None;
    };
    Some(Status {
        state: v
            .get("state")
            .and_then(|s| s.as_str())
            .unwrap_or_default()
            .to_string(),
        last_event: v.get("last_event").and_then(|n| n.as_i64()).unwrap_or(0),
    })
}

fn status(session: &str) -> Option<Status> {
    if session.is_empty() {
        return None;
    }
    let (out, ok) = amx(&["status", session, "--json"]);
    if !ok {
        return None;
    }
    status_in(&out)
}

/// Four base36 characters, from the same random source the claude backend
/// mints uuids out of -- the crate is already a dependency and a v4 carries
/// far more entropy than four digits need.
fn suffix() -> String {
    let mut n = uuid::Uuid::new_v4().as_u128();
    (0..4)
        .map(|_| {
            let c = BASE36[(n % 36) as usize] as char;
            n /= 36;
            c
        })
        .collect()
}

/// Free text as an amx id fragment: lowercase letters, digits and dashes,
/// because an id becomes a directory name under amx's state root and amx
/// refuses anything else at use.
fn slug(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

/// The agent name one dispatch runs under: `wf-<task>-<entropy>`.
///
/// The entropy is the minted session's, so a redispatch of the same task is a
/// new name and the two never collide. The task is spliced in here rather than
/// at mint time because the seam mints before it is told which task it is
/// minting for, and `dispatch` is where the authoritative handle comes from --
/// no adoption dance, just a name known before the pane exists. A wall of
/// `wf-a3k9` rows in `amx ls` would give up the one thing naming was for.
fn name_for(d: &Dispatch) -> String {
    let entropy = d.session.rsplit('-').next().unwrap_or_default();
    let parts = [PREFIX, &slug(&d.task), &slug(entropy)];
    parts
        .iter()
        .filter(|p| !p.is_empty())
        .map(|p| p.to_string())
        .collect::<Vec<_>>()
        .join("-")
}

/// The dispatch argv, as `mem wiki amx-backend` pins it.
///
/// `--no-worktree` because workflow has already cut this task's worktree and
/// the merge gate anchors on its branch; a second one wrapped around it would
/// leave the commits somewhere nothing looks.
fn new_argv(d: &Dispatch, name: &str) -> Vec<String> {
    let path = |p: &Path| p.to_string_lossy().to_string();
    vec![
        "new".to_string(),
        "--name".to_string(),
        name.to_string(),
        "--dir".to_string(),
        path(&d.worktree),
        "--no-worktree".to_string(),
        "--model".to_string(),
        d.model.clone(),
        format!("Read {} and execute it exactly.", path(&d.brief)),
    ]
}

/// The variables a worker must not inherit (spec §1), out of the names the
/// orchestrator is carrying.
///
/// The claude backend strips these with `env -u` inside its template. amx has
/// no template: it snapshots the environment it was spawned with and replays
/// that into the pane, so the scrub has to happen on the command itself or the
/// worker gets every credential this process holds.
///
/// `WORKFLOW_ALLOW_PUSH` and `WORKFLOW_HOOK_SEEN` go with the credentials for
/// the same reason they do there: an orchestrator run under the first would
/// release the pre-push refusal for every worker it dispatched, and the second
/// would tell a worker's first commit that the gate had already run.
fn scrubbed(names: impl IntoIterator<Item = String>) -> Vec<String> {
    const ALWAYS: [&str; 3] = [
        "GITHUB_API_KEY",
        "WORKFLOW_ALLOW_PUSH",
        "WORKFLOW_HOOK_SEEN",
    ];
    const SUFFIXES: [&str; 3] = ["_TOKEN", "_KEY", "_SECRET"];
    const PREFIXES: [&str; 4] = ["GH_", "GITHUB_", "AWS_", "STRIPE_"];

    names
        .into_iter()
        .filter(|name| {
            if ALWAYS.contains(&name.as_str()) {
                return true;
            }
            // The template's two patterns anchor on a variable name's whole
            // charset, so a name with anything else in it is not one of them.
            if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return false;
            }
            SUFFIXES.iter().any(|s| name.ends_with(s))
                || PREFIXES.iter().any(|p| name.starts_with(p))
        })
        .collect()
}

impl WorkerBackend for AmxBackend {
    /// A name amx will take, minted before the task is known. `dispatch`
    /// qualifies it with the task and hands back what it pinned.
    fn mint_session(&self) -> String {
        format!("{PREFIX}-{}", suffix())
    }

    fn dispatch(&self, d: &Dispatch) -> String {
        let name = name_for(d);
        let mut c = Command::new(amx_bin());
        c.args(new_argv(d, &name));
        for (k, v) in &d.env {
            c.env(k, v);
        }
        // What the claude template writes in front of its own command line.
        c.env("WORKFLOW_AGENT", "1");
        for name in scrubbed(std::env::vars().map(|(k, _)| k)) {
            c.env_remove(name);
        }
        // `amx new` prints the id and returns as soon as the pane is up, so
        // there is nothing to detach from and nothing to wait for.
        let _ = c.stdout(Stdio::null()).stderr(Stdio::null()).status();
        name
    }

    fn alive(&self, h: &Handle) -> bool {
        match status(&h.session) {
            Some(s) => !ENDINGS.contains(&s.state.as_str()),
            // No record: at dispatch time the pane is still coming up, so
            // claim alive and let the stall deadline decide. A settled task
            // reads this through `seen` instead, which says gone.
            None => true,
        }
    }

    fn seen(&self, h: &Handle) -> bool {
        // The exit code, not the parse: `status` exits 0 for every agent amx
        // has a record of, and a failure with no record is the one thing that
        // means this session never existed (friction #9F7WT13K).
        !h.session.is_empty() && amx(&["status", &h.session, "--json"]).1
    }

    fn last_activity(&self, h: &Handle) -> i64 {
        let heard = status(&h.session).map(|s| s.last_event).unwrap_or(0);
        heard.max(sys::newest_mtime(&h.worktree))
    }

    /// amx's status carries no token count, so this backend cannot see how
    /// full the window got. None rather than zero: zero would claim the worker
    /// used no context.
    fn context_tokens(&self, _h: &Handle) -> Option<u64> {
        None
    }

    /// The grace is amx's own: `amx stop` asks the pane's process group to
    /// stop, waits for it to finish writing, and only then kills. A worker
    /// started `--no-worktree` has no tree or branch of its own, so stop has
    /// nothing to ask about and nothing to read from stdin.
    fn stop(&self, h: &Handle, _grace_s: i64) {
        if h.session.is_empty() {
            return;
        }
        let _ = amx(&["stop", &h.session]);
    }

    /// Called only once the worker is no longer alive. amx leaves no result
    /// document, so the phase it ended in is the whole of the backend's word;
    /// the status file and the merge gate judge the work.
    fn result(&self, h: &Handle, _out: &Path) -> Outcome {
        Outcome {
            ok: status(&h.session).is_some_and(|s| CLEAN.contains(&s.state.as_str())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Mutex;

    /// One agent as `amx status --json` prints it: amx's `View::json`, with
    /// the fields this backend never reads left in so the parse is exercised
    /// against the document it will really get.
    const STATUS: &str = r#"{
      "id": "wf-t1-a3k9", "state": "working", "evidence": "hooks", "rule": null,
      "age": 12, "since": 1787939694, "last_event": 1787939754, "ended": 0,
      "worked": 7, "seq": 3, "summary": null, "question": null, "options": [],
      "questions": [], "multi": false, "result": null, "source": "payload",
      "exit": null, "kind": null, "pr": [],
      "task": "Read /cache/briefs/t1.md and execute it exactly.",
      "dir": "/state/my project/t1", "worktree": null, "branch": null,
      "base": null, "pane": "%7", "socket": {"name": "default"},
      "session": "685bae4d-35cd-4a63-b50e-686ebcae1aa9", "created": 1787939721
    }"#;

    fn fixture() -> Dispatch {
        Dispatch {
            task: "t1".into(),
            worktree: PathBuf::from("/state/my project/t1"),
            brief: PathBuf::from("/cache/briefs/t1.md"),
            out: PathBuf::from("/runs/t1.json"),
            err: PathBuf::from("/runs/t1.err"),
            pidfile: PathBuf::from("/runs/t1.pid"),
            status: PathBuf::from("/runs/t1.status"),
            rundir: PathBuf::from("/runs"),
            session: "wf-a3k9".into(),
            model: "opus".into(),
            turns: "120".into(),
            env: Vec::new(),
        }
    }

    #[test]
    fn the_status_json_gives_up_the_phase_and_the_last_thing_heard() {
        let s = status_in(STATUS).unwrap();
        assert_eq!(s.state, "working");
        assert_eq!(s.last_event, 1787939754);
        // A document missing what it should carry is still a record.
        assert_eq!(status_in("{}"), Some(Status::default()));
        // Anything that is not one agent's object is not a record at all.
        assert_eq!(status_in("[]"), None);
        assert_eq!(status_in("amx: no agent `nope`"), None);
        assert_eq!(status_in(""), None);
    }

    #[test]
    fn only_a_phase_amx_calls_finished_ends_the_worker() {
        for live in ["starting", "working", "unknown"] {
            assert!(!ENDINGS.contains(&live), "{live} is not an ending");
        }
        for ended in ["waiting", "idle", "done", "failed", "stopped"] {
            assert!(ENDINGS.contains(&ended), "{ended} is an ending");
        }
        // A question is an ending, and it is not a clean one: the worker owes
        // a `blocked` line and a `mem ask` for it.
        assert!(!CLEAN.contains(&"waiting"));
        for ok in ["idle", "done"] {
            assert!(CLEAN.contains(&ok));
        }
        for bad in ["failed", "stopped", "unknown"] {
            assert!(!CLEAN.contains(&bad));
        }
    }

    #[test]
    fn a_minted_name_is_an_id_amx_will_take() {
        let s = AmxBackend.mint_session();
        assert!(s.starts_with("wf-"), "{s}");
        assert_eq!(s.len(), "wf-".len() + 4);
        assert!(
            s.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
            "{s}"
        );
        assert_ne!(s, AmxBackend.mint_session());
    }

    #[test]
    fn the_name_a_dispatch_runs_under_carries_its_task() {
        assert_eq!(name_for(&fixture()), "wf-t1-a3k9");
        // A redispatch mints again, so the entropy moves and the old name is
        // never asked to be free.
        let mut d = fixture();
        d.session = "wf-zz01".into();
        assert_eq!(name_for(&d), "wf-t1-zz01");
        // Nothing amx would refuse gets into a name it has to make a
        // directory out of.
        d.task = "T 1/../x".into();
        assert_eq!(name_for(&d), "wf-t-1-x-zz01");
        d.session = String::new();
        assert_eq!(name_for(&d), "wf-t-1-x");
    }

    #[test]
    fn the_dispatch_argv_is_the_one_the_wiki_pins() {
        assert_eq!(
            new_argv(&fixture(), "wf-t1-a3k9"),
            vec![
                "new",
                "--name",
                "wf-t1-a3k9",
                "--dir",
                "/state/my project/t1",
                "--no-worktree",
                "--model",
                "opus",
                "Read /cache/briefs/t1.md and execute it exactly.",
            ]
        );
    }

    #[test]
    fn the_scrub_takes_what_the_claude_template_takes() {
        let held = [
            "GITHUB_API_KEY",
            "WORKFLOW_ALLOW_PUSH",
            "WORKFLOW_HOOK_SEEN",
            "ANTHROPIC_API_KEY",
            "NPM_TOKEN",
            "MY_SECRET",
            "GH_HOST",
            "AWS_REGION",
            "STRIPE_LIVE",
            // Kept: the worker needs these and none of them is a credential.
            "PATH",
            "HOME",
            "CARGO_TARGET_DIR",
            "WORKFLOW_AGENT",
            "KEYRING",
            "BASH_FUNC_x%%",
        ];
        let mut gone = scrubbed(held.iter().map(|n| n.to_string()));
        gone.sort();
        assert_eq!(
            gone,
            [
                "ANTHROPIC_API_KEY",
                "AWS_REGION",
                "GH_HOST",
                "GITHUB_API_KEY",
                "MY_SECRET",
                "NPM_TOKEN",
                "STRIPE_LIVE",
                "WORKFLOW_ALLOW_PUSH",
                "WORKFLOW_HOOK_SEEN",
            ]
        );
    }

    /// `WORKFLOW_AMX` is process-wide, so the tests that point it at a fake
    /// take turns. A panic inside one must not lock the rest out, hence the
    /// poison is stepped over.
    static ENV: Mutex<()> = Mutex::new(());

    struct Fake {
        dir: PathBuf,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl Fake {
        /// A stand-in for amx that records every call's argv and the
        /// environment it was handed, and answers `status` for one known id
        /// out of [`STATUS`] with `state` replaced.
        fn new(test: &str, state: &str) -> Fake {
            let lock = ENV.lock().unwrap_or_else(|e| e.into_inner());
            let dir = std::env::temp_dir().join(format!("wf-amx-{}-{test}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let d = dir.display();
            std::fs::write(
                dir.join("status.json"),
                STATUS.replace("\"state\": \"working\"", &format!("\"state\": \"{state}\"")),
            )
            .unwrap();
            let bin = dir.join("amx");
            std::fs::write(
                &bin,
                format!(
                    r#"#!/bin/sh
printf '%s\n' "$@" >> '{d}/argv'
env > '{d}/env'
if [ "$1" = status ]; then
  [ "$2" = wf-t1-a3k9 ] || exit 1
  cat '{d}/status.json'
fi
exit 0
"#
                ),
            )
            .unwrap();
            let mut perm = std::fs::metadata(&bin).unwrap().permissions();
            std::os::unix::fs::PermissionsExt::set_mode(&mut perm, 0o755);
            std::fs::set_permissions(&bin, perm).unwrap();
            // SAFETY: every test that touches these holds `ENV`, and nothing
            // else in this crate reads the environment off another thread.
            unsafe {
                std::env::set_var("WORKFLOW_AMX", &bin);
                std::env::set_var("GITHUB_API_KEY", "leak-me");
            }
            Fake { dir, _lock: lock }
        }

        fn handle(&self, session: &str) -> Handle {
            Handle {
                session: session.into(),
                pidfile: self.dir.join("unused.pid"),
                worktree: self.dir.clone(),
            }
        }

        fn read(&self, name: &str) -> String {
            std::fs::read_to_string(self.dir.join(name)).unwrap_or_default()
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            // SAFETY: as above -- the lock is still held until this returns.
            unsafe {
                std::env::remove_var("WORKFLOW_AMX");
                std::env::remove_var("GITHUB_API_KEY");
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn a_dispatch_starts_the_named_agent_and_answers_with_that_name() {
        let fake = Fake::new("dispatch", "working");
        let mut d = fixture();
        d.worktree = fake.dir.clone();
        d.env = vec![("CARGO_TARGET_DIR".into(), "/tmp/target".into())];

        assert_eq!(AmxBackend.dispatch(&d), "wf-t1-a3k9");
        assert_eq!(
            fake.read("argv").lines().collect::<Vec<_>>(),
            new_argv(&d, "wf-t1-a3k9")
        );

        let env = fake.read("env");
        let has = |line: &str| env.lines().any(|l| l == line);
        assert!(has("WORKFLOW_AGENT=1"), "{env}");
        assert!(has("CARGO_TARGET_DIR=/tmp/target"), "{env}");
        assert!(
            !env.lines().any(|l| l.starts_with("GITHUB_API_KEY=")),
            "a credential reached the pane: {env}"
        );
    }

    #[test]
    fn liveness_and_the_record_are_read_off_amx_status() {
        let fake = Fake::new("alive", "working");
        assert!(AmxBackend.alive(&fake.handle("wf-t1-a3k9")));
        assert!(AmxBackend.seen(&fake.handle("wf-t1-a3k9")));
        assert!(
            fake.read("argv").contains("--json"),
            "the stable json is what was asked for"
        );
        // An agent amx has never heard of: not seen, and still claimed alive,
        // because a pane that is coming up has no record yet either.
        assert!(!AmxBackend.seen(&fake.handle("nope")));
        assert!(AmxBackend.alive(&fake.handle("nope")));
        // Never dispatched at all.
        assert!(!AmxBackend.seen(&fake.handle("")));
        assert!(
            AmxBackend
                .context_tokens(&fake.handle("wf-t1-a3k9"))
                .is_none()
        );
    }

    #[test]
    fn a_worker_that_ended_is_not_alive() {
        for (state, alive) in [
            ("starting", true),
            ("working", true),
            ("unknown", true),
            ("waiting", false),
            ("idle", false),
            ("done", false),
            ("failed", false),
            ("stopped", false),
        ] {
            let fake = Fake::new("ended", state);
            assert_eq!(
                AmxBackend.alive(&fake.handle("wf-t1-a3k9")),
                alive,
                "{state}"
            );
        }
    }

    #[test]
    fn the_ending_amx_reports_is_the_backends_whole_word_on_the_result() {
        let out = PathBuf::from("/runs/t1.json");
        for (state, ok) in [
            ("idle", true),
            ("done", true),
            ("waiting", false),
            ("failed", false),
            ("stopped", false),
            ("unknown", false),
        ] {
            let fake = Fake::new("result", state);
            assert_eq!(
                AmxBackend.result(&fake.handle("wf-t1-a3k9"), &out).ok,
                ok,
                "{state}"
            );
        }
        // No record of the session at all is not a clean ending.
        let fake = Fake::new("result", "done");
        assert!(!AmxBackend.result(&fake.handle("nope"), &out).ok);
    }

    #[test]
    fn stopping_a_worker_asks_amx_to_stop_it() {
        let fake = Fake::new("stop", "working");
        AmxBackend.stop(&fake.handle("wf-t1-a3k9"), 10);
        assert_eq!(
            fake.read("argv").lines().collect::<Vec<_>>(),
            ["stop", "wf-t1-a3k9"]
        );
        // Nothing was ever dispatched, so there is nothing to stop.
        AmxBackend.stop(&fake.handle(""), 10);
        assert_eq!(fake.read("argv").lines().count(), 2);
    }

    #[test]
    fn the_last_sign_of_life_is_the_newer_of_amxs_and_the_worktrees() {
        let fake = Fake::new("activity", "working");
        let h = fake.handle("wf-t1-a3k9");
        // The fixture's stamp is in the past, so the scratch worktree amx's
        // own files were just written into is the newer of the two.
        let seen = AmxBackend.last_activity(&h);
        assert!(seen > 1787939754, "{seen}");
        assert_eq!(seen, sys::newest_mtime(&fake.dir));

        // With no worktree to read, amx's stamp is the whole answer.
        let mut gone = fake.handle("wf-t1-a3k9");
        gone.worktree = fake.dir.join("never-made");
        assert_eq!(AmxBackend.last_activity(&gone), 1787939754);
    }
}
