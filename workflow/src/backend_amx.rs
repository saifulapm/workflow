//! The amx worker backend (spec §12b; the design is `mem wiki amx-backend`).
//!
//! Every worker and every reader is an amx agent in a tmux pane, and nothing
//! above [`WorkerBackend`] knows more than that. amx answers all four
//! questions off one verb -- `amx status <id> --json` -- so liveness, the
//! record and the ending are one parse of one document rather than a
//! listing, a transcript and a pidfile.
//!
//! What that document does not carry is what the worker last said and how
//! much context it was carrying. Both are in the claude transcript the pane
//! is running, and the conversation `status` names is what finds the file.
//!
//! `WORKFLOW_AMX` names the binary, defaulting to `amx` on PATH. It is this
//! backend's test seam; `WORKFLOW_WORKER_CMD` selects the process seam
//! instead, and the two mean nothing to each other.

use std::path::Path;
use std::process::{Command, Stdio};

use crate::backend::{
    Consult, Dispatch, Ending, Handle, Outcome, WorkerBackend, last_context_tokens, last_stop_in,
    last_words_in,
};
use crate::{gitcmd, paths, sys};

/// The phases that mean nothing more is coming from this agent.
///
/// `waiting` is one of them: an agent stopped on a question has ended its
/// turn, and the status file protocol is what answers it -- the worker writes
/// `blocked` and asks through `mem ask`.
///
/// The live phases are the ones not here: `starting`, `working`, and
/// `unknown`, which is amx saying it cannot read the pane rather than that the
/// pane is finished. Treating a screen amx cannot account for as an ending
/// would collect a worker mid-turn; left alive, the stall deadline decides it.
const ENDINGS: [&str; 5] = ["waiting", "idle", "done", "failed", "stopped"];

/// The phases that end a turn without an error. Everything else that ends --
/// `waiting` on a question, `failed`, `stopped`, an unreadable screen -- is
/// not a clean ending. What the work was worth is still the status file's and
/// the merge gate's to say.
const CLEAN: [&str; 2] = ["idle", "done"];

/// The evidence words that mean the pane is still there to be read.
const PANE_UP: [&str; 3] = ["hooks", "screen", "unknown"];

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

/// The fields of `amx status --json` this backend reads.
#[derive(Debug, Clone, Default, PartialEq)]
struct Status {
    state: String,
    /// What the phase was read off: `hooks`, `screen` or `unknown` while
    /// the pane stands; `record` once an exit code or a stop ended it,
    /// `gone` when the pane vanished, `parked` when amx released it idle.
    evidence: String,
    last_event: i64,
    /// The claude conversation the pane is running, which is what names the
    /// transcript under the worker's directory. Empty for an agent that never
    /// got as far as one.
    session: String,
    /// The context the worker was carrying, per amx's own reading of its
    /// transcript. Absent (missing key or `null`) on an amx without
    /// `status-context`, which is when the transcript is read instead.
    context: Option<u64>,
    /// The worker's last text, the same way. Absent the same way.
    last_words: Option<String>,
    /// The text of the question the pane is stopped at, off `question.text`.
    /// `null` while nothing is asked.
    question: Option<String>,
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
        evidence: v
            .get("evidence")
            .and_then(|s| s.as_str())
            .unwrap_or_default()
            .to_string(),
        last_event: v.get("last_event").and_then(|n| n.as_i64()).unwrap_or(0),
        session: v
            .get("session")
            .and_then(|s| s.as_str())
            .unwrap_or_default()
            .to_string(),
        context: v.get("context").and_then(|n| n.as_u64()),
        last_words: v
            .get("last_words")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string()),
        question: v
            .get("question")
            .and_then(|q| q.get("text"))
            .and_then(|s| s.as_str())
            .map(|s| s.to_string()),
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

/// Four base36 characters out of a v4 uuid -- the crate is already a
/// dependency and a v4 carries far more entropy than four digits need.
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

/// Whether `name` is a role amx would find for a spawn in `worktree`: the
/// person's `~/.config/amx/agents/<name>.md`, or the project's
/// `.amx/agents/<name>.md`. `mem project set model <name>` may name one --
/// a model, an agent and a brief written down once -- and a dispatch on it
/// goes out as `--role <name>` with no `--model` beside it.
///
/// The project is the one amx asks for: the repository the worktree was cut
/// from (the parent of its git common dir), not the worktree itself, so an
/// untracked role file in the checkout is found from a fresh worktree the
/// way amx will find it. A directory git does not know is its own project.
fn names_a_role(name: &str, worktree: &Path) -> bool {
    if name.is_empty() || name.contains('/') {
        return false;
    }
    let file = format!("{name}.md");
    let project = gitcmd::Git::at(worktree)
        .common_dir()
        .and_then(|d| d.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| worktree.to_path_buf());
    project.join(".amx/agents").join(&file).is_file() || paths::amx_roles().join(&file).is_file()
}

/// The spawn's own words, after the verb and the name, the same on `new` and
/// on `sub`, as `mem wiki amx-backend` pins them.
///
/// `--no-worktree` because workflow has already cut this task's worktree and
/// the merge gate anchors on its branch; a second one wrapped around it would
/// leave the commits somewhere nothing looks. `--role` is the kind of agent
/// this is, whose brief amx puts in front of the task -- or the role the
/// model dial names, which then says the model too. `--effort` only when
/// the run has a level: absent, amx starts the agent on its own default.
fn spawn_argv(d: &Dispatch) -> Vec<String> {
    let path = |p: &Path| p.to_string_lossy().to_string();
    let mut argv = vec![
        "--dir".to_string(),
        path(&d.worktree),
        "--no-worktree".to_string(),
        "--role".to_string(),
    ];
    if names_a_role(&d.model, &d.worktree) {
        argv.push(d.model.clone());
    } else {
        argv.push(d.role.clone());
        argv.push("--model".to_string());
        argv.push(d.model.clone());
    }
    if let Some(level) = &d.effort {
        argv.push("--effort".to_string());
        argv.push(level.clone());
    }
    argv.push(format!("Read {} and execute it exactly.", path(&d.brief)));
    argv
}

/// The dispatch argv for an agent with no parent: `amx new --name <name> ...`.
///
/// `new` is a root whatever the pane's `AMX_ID` says, and only `amx sub`
/// records a parent, so a run started inside an amx pane still dispatches its
/// workers at depth zero rather than a depth deeper than it meant -- deep
/// enough, one reader later, to meet amx's `subagent_depth`.
fn new_argv(d: &Dispatch, name: &str) -> Vec<String> {
    let mut argv = vec!["new".to_string(), "--name".to_string(), name.to_string()];
    argv.extend(spawn_argv(d));
    argv
}

/// The dispatch argv for a child: `amx sub --bg --json --name <name>
/// --parent <id> ...`. `--bg` because the run keeps its own poll and needs
/// the id now, not at the end; `--json` because that is where the id is.
fn sub_bg_argv(d: &Dispatch, name: &str, parent: &str) -> Vec<String> {
    let mut argv = vec![
        "sub".to_string(),
        "--bg".to_string(),
        "--json".to_string(),
        "--name".to_string(),
        name.to_string(),
        "--parent".to_string(),
        parent.to_string(),
    ];
    argv.extend(spawn_argv(d));
    argv
}

/// The consult argv: `amx sub --json --timeout <s> --name <name> [--parent
/// <id>] ...`, one call that starts the agent and waits for its answer.
fn sub_argv(d: &Dispatch, name: &str, timeout_s: i64) -> Vec<String> {
    let mut argv = vec![
        "sub".to_string(),
        "--json".to_string(),
        "--timeout".to_string(),
        timeout_s.max(1).to_string(),
        "--name".to_string(),
        name.to_string(),
    ];
    if let Some(parent) = &d.parent {
        argv.push("--parent".to_string());
        argv.push(parent.clone());
    }
    argv.extend(spawn_argv(d));
    argv
}

/// The two fields of `amx sub --json` this backend reads: the child's id and
/// its answer, `null` for a turn that gave none.
fn sub_json(json: &str) -> Option<(String, String)> {
    let serde_json::Value::Object(v) = serde_json::from_str::<serde_json::Value>(json).ok()? else {
        return None;
    };
    let id = v.get("id")?.as_str()?.to_string();
    let answer = v
        .get("answer")
        .and_then(|a| a.as_str())
        .unwrap_or_default()
        .to_string();
    Some((id, answer))
}

/// `amx sub`'s exit code as an ending: `result`'s codes, 0 an answer, 2 the
/// child is asking a question, 3 the deadline, and anything else -- 1 failed
/// or stopped, 64 usage, no code at all -- unclean.
fn ending_of(code: Option<i32>) -> Ending {
    match code {
        Some(0) => Ending::Answered,
        Some(2) => Ending::Blocked,
        Some(3) => Ending::TimedOut,
        _ => Ending::Unclean,
    }
}

/// The variables a worker must not inherit (spec §1), out of the names the
/// orchestrator is carrying.
///
/// amx snapshots the environment it was spawned with and replays that into
/// the pane, so the scrub happens on the command itself or the worker gets
/// every credential this process holds.
///
/// `WORKFLOW_ALLOW_PUSH` and `WORKFLOW_HOOK_SEEN` go with the credentials:
/// an orchestrator run under the first would
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

/// Run one spawning verb -- `new`, `sub` -- with the dispatch's environment:
/// its own pairs set, the worker mark on, the credentials of spec 1 scrubbed,
/// stderr where the dispatch was told to put it. None when amx could not be
/// run at all.
///
/// A launch amx refuses says why on stderr and nowhere else, and for a
/// reader that line is the whole diagnosis -- so it goes to `Dispatch::err`
/// rather than to /dev/null.
fn spawn(argv: &[String], d: &Dispatch) -> Option<std::process::Output> {
    let mut c = Command::new(amx_bin());
    c.args(argv);
    for (k, v) in &d.env {
        c.env(k, v);
    }
    // How a worker's hooks and mem know they are a worker's.
    c.env("WORKFLOW_AGENT", "1");
    for name in scrubbed(std::env::vars().map(|(k, _)| k)) {
        c.env_remove(name);
    }
    let err = match std::fs::File::create(&d.err) {
        Ok(f) => Stdio::from(f),
        Err(_) => Stdio::null(),
    };
    c.stdout(Stdio::piped()).stderr(err).output().ok()
}

impl WorkerBackend for AmxBackend {
    /// A name amx will take, minted before the task is known. `dispatch`
    /// qualifies it with the task and hands back what it pinned.
    fn mint_session(&self) -> String {
        format!("{PREFIX}-{}", suffix())
    }

    fn dispatch(&self, d: &Dispatch) -> String {
        let name = name_for(d);
        let argv = match &d.parent {
            Some(parent) => sub_bg_argv(d, &name, parent),
            None => new_argv(d, &name),
        };
        // `amx new` prints the id and returns as soon as the pane is up, so
        // there is nothing to detach from and nothing to wait for; `sub --bg
        // --json` says the same id in its object. A launch amx refused (exit
        // 2 at its cap, 64 for a parent it has no record of, 1 for anything
        // else) started no agent: no handle, and the run reads the refusal
        // off `err`.
        let Some(out) = spawn(&argv, d) else {
            return String::new();
        };
        if !out.status.success() {
            return String::new();
        }
        match d.parent {
            None => name,
            // The id is the name asked for, or the record is not the one
            // this dispatch will be asked after by -- and a child amx did
            // start under some other id is stopped rather than left running
            // in the tree with nothing holding its handle.
            Some(_) => match sub_json(&String::from_utf8_lossy(&out.stdout)) {
                Some((id, _)) if id == name => name,
                Some((id, _)) => {
                    let _ = amx(&["stop", &id]);
                    String::new()
                }
                None => String::new(),
            },
        }
    }

    /// One `amx sub --json --timeout`: the agent, its turn and its ending in
    /// one call, the answer off the object and the ending off the exit code.
    /// amx has stopped nothing when the deadline passes -- `result` gave up
    /// waiting, the pane stands -- so the stop is this backend's, as it is
    /// for every ending: a consulted agent has one answer to give.
    fn consult(&self, d: &Dispatch, timeout_s: i64) -> Consult {
        let name = name_for(d);
        let refused = Consult {
            ending: Ending::Unclean,
            answer: String::new(),
            session: String::new(),
            question: String::new(),
        };
        let Some(out) = spawn(&sub_argv(d, &name, timeout_s), d) else {
            return refused;
        };
        let Some((id, answer)) = sub_json(&String::from_utf8_lossy(&out.stdout)) else {
            // amx prints the object on every ending it saw; a clean exit
            // with none is a child that may be standing under the name
            // asked for, so it is stopped before this says nothing ran.
            if out.status.success() {
                let _ = amx(&["stop", &name]);
            }
            return refused;
        };
        let h = Handle {
            session: id.clone(),
            pidfile: d.pidfile.clone(),
            worktree: d.worktree.clone(),
        };
        let ending = ending_of(out.status.code());
        // The question lives on the pane and amx forgets it with the phase
        // the stop brings, so it is read first: exit 2 is exactly the ending
        // whose wording the caller needs.
        let question = match ending {
            Ending::Blocked => self.question(&h),
            _ => String::new(),
        };
        self.stop(&h, (timeout_s / 2).clamp(1, 30));
        Consult {
            ending,
            answer,
            session: id,
            question,
        }
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

    /// A pane still standing. `record` and `gone` are an agent amx remembers
    /// and nothing more -- the transcript-and-no-row of frictions #B3391C6H
    /// and #QT1PDNRK -- and `parked` is a pane amx released after an hour
    /// idle; none of those is a session the usage limit is holding.
    fn listed(&self, h: &Handle) -> bool {
        status(&h.session).is_some_and(|s| PANE_UP.contains(&s.evidence.as_str()))
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

    /// `status --json`'s own reading when amx ships one; else the transcript
    /// of the conversation amx names, the same way the last words are. None
    /// rather than zero when there is nothing to read: zero would claim the
    /// worker used no context.
    fn context_tokens(&self, h: &Handle) -> Option<u64> {
        let s = status(&h.session)?;
        if s.context.is_some() {
            return s.context;
        }
        if s.session.is_empty() {
            return None;
        }
        let path = paths::transcript_path(&h.worktree, &s.session);
        last_context_tokens(&std::fs::read_to_string(path).ok()?)
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

    /// `amx send <id> <text>`: a bracketed paste into the pane, confirmed
    /// against the vendor's own prompt event. amx refuses at a question
    /// (exit 2), on a pane it let go or an agent that ended (1), and says
    /// failure when nothing confirmed the paste within its window -- every
    /// one of those is a `false` here, and the run takes it from there.
    fn send(&self, h: &Handle, text: &str) -> bool {
        !h.session.is_empty() && amx(&["send", &h.session, text]).1
    }

    /// Called only once the worker is no longer alive. amx leaves no result
    /// document, so the phase it ended in is the whole of the backend's word;
    /// the status file and the merge gate judge the work.
    fn result(&self, h: &Handle, _out: &Path) -> Outcome {
        Outcome {
            ok: status(&h.session).is_some_and(|s| CLEAN.contains(&s.state.as_str())),
        }
    }

    /// `status --json`'s own reading when amx ships one. Else, a worker under
    /// amx is a claude session in a pane and leaves that session's
    /// transcript; the id naming it is amx's to give, and `amx status --json`
    /// is what gives it. An agent whose status names no conversation has no
    /// last words: the reading ended before one existed, and the transcripts
    /// standing under the directory belong to other panes.
    fn last_words(&self, h: &Handle) -> String {
        let Some(s) = status(&h.session) else {
            return String::new();
        };
        if let Some(words) = s.last_words {
            return words;
        }
        if s.session.is_empty() {
            return String::new();
        }
        let path = paths::transcript_path(&h.worktree, &s.session);
        last_words_in(&std::fs::read_to_string(path).unwrap_or_default())
    }

    /// The transcript amx names for this agent, read for how its last turn
    /// stopped. amx has no word of its own on this: `status --json` reports a
    /// phase, and a turn that ended on a completion budget spent reasoning
    /// ends in the same `done` as one that answered.
    fn last_stop(&self, h: &Handle) -> Option<(String, u64)> {
        let s = status(&h.session)?;
        if s.session.is_empty() {
            return None;
        }
        let path = paths::transcript_path(&h.worktree, &s.session);
        last_stop_in(&std::fs::read_to_string(path).ok()?)
    }

    /// `amx logs <id>`: once the pane is gone this is the recorded answer, or
    /// what the pane's boot kept of a session that never announced a
    /// transcript -- the one account a schema death on the first request
    /// leaves. Called only once the worker is no longer alive and has no
    /// last words, so a live screen is never read as an ending.
    fn dying_words(&self, h: &Handle) -> String {
        if h.session.is_empty() {
            return String::new();
        }
        amx(&["logs", &h.session]).0.trim_end().to_string()
    }

    /// What amx read off the pane: a question drawn in front of the session
    /// -- claude's folder-trust screen, a permission prompt -- is on the
    /// screen and nowhere else, and amx is what reads screens.
    fn question(&self, h: &Handle) -> String {
        status(&h.session)
            .and_then(|s| s.question)
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Mutex;

    /// The conversation [`STATUS`] says its agent is running.
    const SESSION: &str = "685bae4d-35cd-4a63-b50e-686ebcae1aa9";

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
            parent: None,
            role: "worker".into(),
            model: "opus".into(),
            effort: None,
            turns: "120".into(),
            env: Vec::new(),
        }
    }

    #[test]
    fn the_status_json_gives_up_the_phase_and_the_last_thing_heard() {
        let s = status_in(STATUS).unwrap();
        assert_eq!(s.state, "working");
        assert_eq!(s.evidence, "hooks");
        assert_eq!(s.last_event, 1787939754);
        // The conversation the pane is running, which is the transcript this
        // backend reads a reader's last words out of.
        assert_eq!(s.session, SESSION);
        // A document missing what it should carry is still a record.
        assert_eq!(status_in("{}"), Some(Status::default()));
        // Anything that is not one agent's object is not a record at all.
        assert_eq!(status_in("[]"), None);
        assert_eq!(status_in("amx: no agent `nope`"), None);
        assert_eq!(status_in(""), None);
    }

    #[test]
    fn status_in_reads_context_and_last_words_when_amx_ships_them() {
        // An amx before status-context: the keys are not there at all, and
        // both answer None rather than a guess.
        let s = status_in(STATUS).unwrap();
        assert_eq!(s.context, None);
        assert_eq!(s.last_words, None);

        // An amx with status-context.
        let with_fields = STATUS.replace(
            "\"created\": 1787939721",
            "\"created\": 1787939721, \"context\": 4213, \"last_words\": \"done already\"",
        );
        let s = status_in(&with_fields).unwrap();
        assert_eq!(s.context, Some(4213));
        assert_eq!(s.last_words, Some("done already".to_string()));

        // A context of `null` reads the same as the key being absent.
        let null_context = STATUS.replace(
            "\"created\": 1787939721",
            "\"created\": 1787939721, \"context\": null",
        );
        assert_eq!(status_in(&null_context).unwrap().context, None);
    }

    #[test]
    fn status_in_reads_the_question_the_pane_is_stopped_at() {
        // `"question": null` is the fixture's own: nothing asked.
        assert_eq!(status_in(STATUS).unwrap().question, None);
        let asked = STATUS.replace(
            "\"question\": null",
            "\"question\": {\"text\": \"Quick safety check: Is this a project you created or one you trust?\", \
             \"options\": [\"Yes, I trust this folder\", \"No, exit\"], \"kind\": \"question\"}",
        );
        assert_eq!(
            status_in(&asked).unwrap().question,
            Some("Quick safety check: Is this a project you created or one you trust?".to_string())
        );
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
                "--role",
                "worker",
                "--model",
                "opus",
                "Read /cache/briefs/t1.md and execute it exactly.",
            ]
        );
    }

    #[test]
    fn a_model_that_names_a_role_goes_out_as_that_role_and_no_model() {
        let fake = Fake::new("role", "working");
        let mut d = fixture();
        d.worktree = fake.dir.clone();
        d.model = "glm-worker".into();
        // Nothing by that name anywhere: an opaque model name, on the kind's
        // own role.
        let argv = new_argv(&d, "wf-t1-a3k9");
        assert!(argv.contains(&"--model".to_string()), "{argv:?}");
        assert_eq!(
            argv[argv.iter().position(|a| a == "--role").unwrap() + 1],
            "worker"
        );

        // The person's role file, where amx reads it.
        let personal = paths::amx_roles();
        std::fs::create_dir_all(&personal).unwrap();
        std::fs::write(personal.join("glm-worker.md"), "---\nmodel: pi:glm\n---\n").unwrap();
        let argv = new_argv(&d, "wf-t1-a3k9");
        assert!(!argv.contains(&"--model".to_string()), "{argv:?}");
        assert_eq!(
            argv[argv.iter().position(|a| a == "--role").unwrap() + 1],
            "glm-worker"
        );

        // The project's counts the same: the fixture is no git repository,
        // so it is its own project and the file is read from the tree.
        std::fs::remove_file(personal.join("glm-worker.md")).unwrap();
        d.model = "repo-worker".into();
        let project = fake.dir.join(".amx/agents");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("repo-worker.md"), "---\n---\n").unwrap();
        let argv = new_argv(&d, "wf-t1-a3k9");
        assert_eq!(
            argv[argv.iter().position(|a| a == "--role").unwrap() + 1],
            "repo-worker"
        );
        // A vendor-qualified model is never a file name.
        d.model = "pi:opencode-go/glm-5.3".into();
        assert!(new_argv(&d, "wf-t1-a3k9").contains(&"--model".to_string()));
    }

    #[test]
    fn a_dispatch_with_a_parent_is_a_sub_in_the_background() {
        let mut d = fixture();
        d.parent = Some("wf-t1-zz01".into());
        let argv = sub_bg_argv(&d, "wf-t1-a3k9", "wf-t1-zz01");
        assert_eq!(
            &argv[..7],
            [
                "sub",
                "--bg",
                "--json",
                "--name",
                "wf-t1-a3k9",
                "--parent",
                "wf-t1-zz01"
            ]
        );
        // The same words after the verb and the name as `new` has.
        assert_eq!(argv[7..], spawn_argv(&d));

        let fake = Fake::new("child", "working");
        d.worktree = fake.dir.clone();
        d.err = fake.dir.join("t1.err");
        assert_eq!(AmxBackend.dispatch(&d), "wf-t1-a3k9");
        assert_eq!(
            fake.read("argv").lines().collect::<Vec<_>>(),
            sub_bg_argv(&d, "wf-t1-a3k9", "wf-t1-zz01")
        );
        // An id that is not the name asked for is no handle: the record amx
        // made is not the one the run would be asking after, and the child it
        // started is stopped rather than left running with nothing holding it.
        std::fs::write(fake.dir.join("sub-name"), "read-the-brief-q9").unwrap();
        assert_eq!(AmxBackend.dispatch(&d), "");
        assert!(
            fake.read("argv").contains("stop\nread-the-brief-q9\n"),
            "{}",
            fake.read("argv")
        );
        // A parent amx has no record of is a refusal (64), and no handle.
        std::fs::remove_file(fake.dir.join("sub-name")).unwrap();
        std::fs::write(fake.dir.join("sub-exit"), "64").unwrap();
        assert_eq!(AmxBackend.dispatch(&d), "");
    }

    #[test]
    fn a_consult_is_one_sub_call_and_its_exit_code_is_the_ending() {
        let mut d = fixture();
        let argv = sub_argv(&d, "wf-t1-a3k9", 900);
        assert_eq!(
            &argv[..6],
            ["sub", "--json", "--timeout", "900", "--name", "wf-t1-a3k9"]
        );
        assert_eq!(argv[6..], spawn_argv(&d));
        // A consult keeps the ambient `AMX_ID`: `workflow advise` runs in the
        // worker's pane and rides that id onto the record as the advisor's
        // parent, which is what a `sub` with no `--parent` does.
        assert!(!argv.contains(&"--no-parent".to_string()), "{argv:?}");
        d.parent = Some("wf-t1-zz01".into());
        assert_eq!(
            &sub_argv(&d, "wf-t1-a3k9", 900)[6..8],
            ["--parent", "wf-t1-zz01"]
        );
        d.parent = None;

        for (code, ending) in [
            (0, Ending::Answered),
            (1, Ending::Unclean),
            (2, Ending::Blocked),
            (3, Ending::TimedOut),
            (64, Ending::Unclean),
        ] {
            let fake = Fake::new("consult", "done");
            d.worktree = fake.dir.clone();
            d.err = fake.dir.join("t1.err");
            std::fs::write(fake.dir.join("sub-exit"), code.to_string()).unwrap();
            let c = AmxBackend.consult(&d, 900);
            assert_eq!(c.ending, ending, "exit {code}");
            assert_eq!(c.session, "wf-t1-a3k9");
            assert_eq!(c.answer, "the answer");
            assert_eq!(c.question, "", "exit {code}: nothing asked");
            // One sub, then the stop: the consulted agent has said its piece.
            let calls: Vec<String> = fake
                .read("argv")
                .lines()
                .filter(|l| *l == "sub" || *l == "stop")
                .map(str::to_string)
                .collect();
            assert_eq!(calls, ["sub", "stop"], "exit {code}");
        }
    }

    #[test]
    fn a_blocked_consult_carries_the_question_read_before_the_stop() {
        let fake = Fake::new("blocked", "waiting");
        let path = fake.dir.join("status.json");
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(
            &path,
            text.replace(
                "\"question\": null",
                "\"question\": {\"text\": \"Is this a project you trust?\", \"options\": [\"Yes\", \"No\"]}",
            ),
        )
        .unwrap();
        std::fs::write(fake.dir.join("sub-exit"), "2").unwrap();
        let mut d = fixture();
        d.worktree = fake.dir.clone();
        d.err = fake.dir.join("t1.err");
        let c = AmxBackend.consult(&d, 900);
        assert_eq!(c.ending, Ending::Blocked);
        assert_eq!(c.question, "Is this a project you trust?");
        // status (the question) was asked before stop.
        let calls: Vec<String> = fake
            .read("argv")
            .lines()
            .filter(|l| ["sub", "status", "stop"].contains(l))
            .map(str::to_string)
            .collect();
        assert_eq!(calls, ["sub", "status", "stop"]);
    }

    #[test]
    fn the_sub_object_gives_up_the_id_and_the_answer() {
        assert_eq!(
            sub_json(
                r#"{"id":"wf-x","parent":"wf-p","phase":"done","answer":"ship","evidence":"record"}"#
            ),
            Some(("wf-x".into(), "ship".into()))
        );
        assert_eq!(
            sub_json(r#"{"id":"wf-x","answer":null}"#),
            Some(("wf-x".into(), String::new()))
        );
        assert_eq!(sub_json("amx sub: no agent `nope`"), None);
        assert_eq!(sub_json("{}"), None);
        assert_eq!(ending_of(None), Ending::Unclean);
    }

    #[test]
    fn the_role_is_the_kind_of_agent_the_dispatch_is() {
        let mut d = fixture();
        d.role = "reader".into();
        let argv = new_argv(&d, "wf-t1-review-a3k9");
        assert_eq!(
            argv[argv.iter().position(|a| a == "--role").unwrap() + 1],
            "reader"
        );
    }

    #[test]
    fn dying_words_are_what_amx_logs_prints() {
        let fake = Fake::new("dying", "failed");
        assert_eq!(
            AmxBackend.dying_words(&fake.handle("wf-t1-a3k9")),
            "amx logs: wf-t1-a3k9"
        );
        assert_eq!(
            fake.read("argv").lines().collect::<Vec<_>>(),
            ["logs", "wf-t1-a3k9"]
        );
        // An agent amx has no record of, and one never dispatched, left none.
        assert_eq!(AmxBackend.dying_words(&fake.handle("nope")), "");
        assert_eq!(AmxBackend.dying_words(&fake.handle("")), "");
    }

    #[test]
    fn the_effort_dial_goes_on_the_argv_only_when_the_run_has_one() {
        let mut d = fixture();
        d.effort = Some("max".into());
        let argv = new_argv(&d, "wf-t1-a3k9");
        assert_eq!(
            &argv[argv.len() - 3..],
            [
                "--effort",
                "max",
                "Read /cache/briefs/t1.md and execute it exactly."
            ]
        );
        assert!(!new_argv(&fixture(), "wf-t1-a3k9").contains(&"--effort".to_string()));
    }

    #[test]
    fn the_scrub_takes_every_credential_shape_of_spec_1() {
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
        home: Option<std::ffi::OsString>,
        config_home: Option<std::ffi::OsString>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl Fake {
        /// A stand-in for amx that records every call's argv and the
        /// environment it was handed, and answers `status` for one known id
        /// out of [`STATUS`] with `state` replaced.
        ///
        /// `HOME` moves into the same directory, so the transcripts
        /// [`paths::transcript_path`] resolves are the fixture's and never
        /// the machine's.
        fn new(test: &str, state: &str) -> Fake {
            let lock = ENV.lock().unwrap_or_else(|e| e.into_inner());
            let home = std::env::var_os("HOME");
            let config_home = std::env::var_os("XDG_CONFIG_HOME");
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
echo "amx: $1: no capacity" >&2
[ "$1" = new ] && [ -f '{d}/refuse' ] && exit 2
[ "$1" = send ] && [ -f '{d}/refuse' ] && exit 2
if [ "$1" = status ] || [ "$1" = logs ]; then
  [ "$2" = wf-t1-a3k9 ] || exit 1
fi
[ "$1" = logs ] && echo "amx logs: $2"
[ "$1" = status ] && cat '{d}/status.json'
if [ "$1" = sub ]; then
  name=; prev=
  for a in "$@"; do [ "$prev" = --name ] && name=$a; prev=$a; done
  [ -f '{d}/sub-name' ] && name=$(cat '{d}/sub-name')
  printf '{{"id":"%s","parent":null,"phase":"done","answer":"the answer","evidence":"record"}}\n' "$name"
  [ -f '{d}/sub-exit' ] && exit "$(cat '{d}/sub-exit')"
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
                std::env::set_var("HOME", &dir);
                std::env::remove_var("XDG_CONFIG_HOME");
            }
            Fake {
                dir,
                home,
                config_home,
                _lock: lock,
            }
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

        /// One conversation file where [`paths::transcript_path`] will look
        /// for it: under the fixture's HOME, in the slug of the worktree the
        /// handles are cut from.
        fn transcript(&self, session: &str, body: &str) {
            let path = paths::transcript_path(&self.dir, session);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }

        /// The same status with no conversation named -- an agent amx has a
        /// record of whose pane never got as far as one.
        fn nameless(&self) {
            let path = self.dir.join("status.json");
            let text = std::fs::read_to_string(&path).unwrap();
            std::fs::write(&path, text.replace(SESSION, "")).unwrap();
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            // SAFETY: as above -- the lock is still held until this returns.
            unsafe {
                std::env::remove_var("WORKFLOW_AMX");
                std::env::remove_var("GITHUB_API_KEY");
                match &self.home {
                    Some(home) => std::env::set_var("HOME", home),
                    None => std::env::remove_var("HOME"),
                }
                match &self.config_home {
                    Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                    None => std::env::remove_var("XDG_CONFIG_HOME"),
                }
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn a_dispatch_starts_the_named_agent_and_answers_with_that_name() {
        let fake = Fake::new("dispatch", "working");
        let mut d = fixture();
        d.worktree = fake.dir.clone();
        d.err = fake.dir.join("t1.err");
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
        // What amx said on its way out is the only account of a launch that
        // was refused, so it goes where the dispatch was told to put it.
        assert_eq!(fake.read("t1.err").trim(), "amx: new: no capacity");
    }

    #[test]
    fn a_readers_last_words_come_off_the_conversation_amx_names() {
        let fake = Fake::new("words", "failed");
        let h = fake.handle("wf-t1-a3k9");
        // The conversation is named but has not been written yet.
        assert_eq!(AmxBackend.last_words(&h), "");

        fake.transcript(
            SESSION,
            "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\
             \"content\":[{\"type\":\"text\",\"text\":\"You've reached your Fable limit.\"}]}}\n",
        );
        assert_eq!(
            AmxBackend.last_words(&h),
            "You've reached your Fable limit."
        );

        // An agent amx has no record of names no conversation to read.
        assert_eq!(AmxBackend.last_words(&fake.handle("nope")), "");
        // Nor does a record that carries no session: the transcript on disk
        // belongs to some other pane, and guessing at it would put another
        // reader's words in this task's account of itself.
        fake.nameless();
        assert_eq!(AmxBackend.last_words(&h), "");
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
        // The window is read off the conversation amx names: nothing to
        // read is None, never zero.
        let h = fake.handle("wf-t1-a3k9");
        assert!(AmxBackend.context_tokens(&h).is_none());
        fake.transcript(
            SESSION,
            "{\"type\":\"assistant\",\"message\":{\"usage\":{\"input_tokens\":3,\"cache_read_input_tokens\":1200}}}\n",
        );
        assert_eq!(AmxBackend.context_tokens(&h), Some(1203));
    }

    #[test]
    fn context_and_last_words_come_off_the_json_before_the_transcript() {
        let fake = Fake::new("json-fields", "idle");
        let path = fake.dir.join("status.json");
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(
            &path,
            text.replace(
                "\"created\": 1787939721",
                "\"created\": 1787939721, \"context\": 4213, \"last_words\": \"done already\"",
            ),
        )
        .unwrap();
        let h = fake.handle("wf-t1-a3k9");
        // No transcript on disk at all -- the JSON field is the whole answer.
        assert_eq!(AmxBackend.context_tokens(&h), Some(4213));
        assert_eq!(AmxBackend.last_words(&h), "done already");
    }

    #[test]
    fn a_readers_question_comes_off_amx_status() {
        let fake = Fake::new("question", "waiting");
        let h = fake.handle("wf-t1-a3k9");
        // Waiting, but the fixture asks nothing.
        assert_eq!(AmxBackend.question(&h), "");
        let path = fake.dir.join("status.json");
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(
            &path,
            text.replace(
                "\"question\": null",
                "\"question\": {\"text\": \"Do you want to proceed?\", \"options\": [\"Yes\", \"No\"]}",
            ),
        )
        .unwrap();
        assert_eq!(AmxBackend.question(&h), "Do you want to proceed?");
        // An agent amx has no record of asks nothing.
        assert_eq!(AmxBackend.question(&fake.handle("nope")), "");
    }

    #[test]
    fn listed_means_the_pane_is_still_standing() {
        for (evidence, listed) in [
            ("hooks", true),
            ("screen", true),
            ("unknown", true),
            ("record", false),
            ("gone", false),
            ("parked", false),
        ] {
            let fake = Fake::new("listed", "idle");
            let path = fake.dir.join("status.json");
            let text = std::fs::read_to_string(&path).unwrap();
            std::fs::write(
                &path,
                text.replace(
                    "\"evidence\": \"hooks\"",
                    &format!("\"evidence\": \"{evidence}\""),
                ),
            )
            .unwrap();
            let h = fake.handle("wf-t1-a3k9");
            assert_eq!(AmxBackend.listed(&h), listed, "{evidence}");
            // Seen either way: amx has the record.
            assert!(AmxBackend.seen(&h), "{evidence}");
        }
        let fake = Fake::new("listed", "idle");
        assert!(!AmxBackend.listed(&fake.handle("nope")));
    }

    #[test]
    fn a_launch_amx_refuses_hands_back_no_handle() {
        let fake = Fake::new("refused", "working");
        std::fs::write(fake.dir.join("refuse"), "").unwrap();
        let mut d = fixture();
        d.worktree = fake.dir.clone();
        d.err = fake.dir.join("t1.err");
        assert_eq!(AmxBackend.dispatch(&d), "");
        assert_eq!(fake.read("t1.err").trim(), "amx: new: no capacity");
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
    fn a_message_goes_through_amx_send_and_its_exit_code_is_the_answer() {
        let fake = Fake::new("send", "idle");
        assert!(AmxBackend.send(&fake.handle("wf-t1-a3k9"), "Read the brief again."));
        assert_eq!(
            fake.read("argv").lines().collect::<Vec<_>>(),
            ["send", "wf-t1-a3k9", "Read the brief again."]
        );
        std::fs::write(fake.dir.join("refuse"), "").unwrap();
        assert!(!AmxBackend.send(&fake.handle("wf-t1-a3k9"), "again"));
        assert!(!AmxBackend.send(&fake.handle(""), "nobody"));
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
