//! The worker backend seam (spec §12b).
//!
//! amx is the execution and visibility substrate, not an orchestrator: `run`
//! dispatches *onto* a backend and keeps every policy decision -- waves,
//! ownership, the merge gate -- to itself. The `claude` backend below
//! launches every worker as a `claude --bg` session: visible in the agents
//! view, attachable, ended with `claude stop`, answered for by
//! `claude agents --json`. A custom `WORKFLOW_WORKER_CMD` template keeps the
//! legacy process semantics (pidfile, signals, result document) -- that is
//! the test seam. An `amx` backend maps the same four questions onto
//! `amx new` / `amx ls` / `amx result` / `amx stop` with nothing above it
//! changing.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{paths, sys};

/// Everything one attempt at a task needs. The names are the template's
/// placeholders (spec §8.3).
#[derive(Debug, Clone)]
pub struct Dispatch {
    pub task: String,
    pub worktree: PathBuf,
    pub brief: PathBuf,
    pub out: PathBuf,
    pub err: PathBuf,
    pub pidfile: PathBuf,
    pub status: PathBuf,
    pub rundir: PathBuf,
    pub session: String,
    pub model: String,
    pub turns: String,
    pub env: Vec<(String, String)>,
}

/// What is left of a dispatch once it is running: enough to ask after it and to
/// stop it.
#[derive(Debug, Clone)]
pub struct Handle {
    pub session: String,
    pub pidfile: PathBuf,
    pub worktree: PathBuf,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Outcome {
    /// The worker finished and said so without an error.
    pub ok: bool,
}

pub trait WorkerBackend {
    /// A fresh handle for one dispatch. A redispatch mints a new one.
    fn mint_session(&self) -> String;
    /// Start the worker and answer with the handle it can be asked after by.
    /// Detached: this returns as soon as it is going.
    ///
    /// The answer is not always the minted one. `claude --bg` mints its own
    /// session id and says so on stderr -- `--session-id` is honoured only
    /// with `--resume` -- so the handle a backend hands back after launch is
    /// the authoritative one, and it is what gets recorded.
    fn dispatch(&self, d: &Dispatch) -> String;
    /// Is anything of this worker still running?
    fn alive(&self, h: &Handle) -> bool;
    /// Does the backend hold any record of this session at all -- a listing
    /// row, a process carrying its id, a transcript, a pidfile? Distinct from
    /// `alive`: a session can be seen and ended. At dispatch time no record
    /// means still launching; at adoption, with the run that recorded the
    /// session dead, it means the session never existed (friction #9F7WT13K).
    fn seen(&self, h: &Handle) -> bool;
    /// The most recent sign of life the backend can see, in epoch seconds.
    /// The worker's own status file is the orchestrator's signal, not the
    /// backend's, and is counted on top of this.
    fn last_activity(&self, h: &Handle) -> i64;
    /// The context the worker was carrying at its last turn, in tokens, when
    /// the backend can see it. Reported and never enforced (ruling #D7A4T2CH):
    /// the plan is flat-rate, so the managed resource is the window, and a task
    /// that ends near a full one was cut too big.
    fn context_tokens(&self, h: &Handle) -> Option<u64>;
    /// Stop the worker and everything it started.
    fn stop(&self, h: &Handle, grace_s: i64);
    /// What the worker left behind: a print-mode result document at `out`, or
    /// the agents list's word on the session named by the handle.
    fn result(&self, h: &Handle, out: &Path) -> Outcome;
}

/// The dispatch template (spec §8.3, amended: workers are `claude --bg`
/// sessions, never print mode -- they appear in `claude agents`, can be
/// attached, and end as resident idle sessions). Every placeholder sits where
/// a single shell-quoted word is legal -- at the outer level, or as a
/// positional argument to the inner `sh -c` -- so a project whose path has a
/// space in it dispatches like any other.
///
/// The `env -u` sweep is the credential scrub of spec §1. The two workflow
/// variables go with it: an orchestrator run under WORKFLOW_ALLOW_PUSH would
/// otherwise release the pre-push refusal for every worker it dispatches, and an
/// inherited WORKFLOW_HOOK_SEEN would tell a worker's first commit that the gate
/// had already run.
///
/// No setsid and no pidfile: `claude --bg` hands the session to its own
/// service and returns. It also mints its own session id: `--session-id` is
/// honoured only alongside `--resume`, and passing it to `--bg` earns a
/// warning on stderr and nothing else. So the flag is not here, and the handle
/// comes back the other way -- `--bg` prints the id it chose, and
/// [`ClaudeBackend::adopt`] reads it off `{out}`.
///
/// No `--max-budget-usd` either: the plan is flat-rate, so a dollar ceiling
/// guards a constraint that does not exist, and its enforcement outside print
/// mode was never verified anyway (ruling #D7A4T2CH). The deadline, the worker
/// cap and one-task briefs are the bounds that hold.
pub const WORKER_CMD_DEFAULT: &str = r#"cd {worktree} && WORKFLOW_AGENT=1 sh -c '\
  exec env -u GITHUB_API_KEY -u WORKFLOW_ALLOW_PUSH -u WORKFLOW_HOOK_SEEN \
  $(env | grep -oE "^[A-Za-z0-9_]*(_TOKEN|_KEY|_SECRET)=|^(GH_|GITHUB_|AWS_|STRIPE_)[A-Za-z0-9_]*=" | sed "s/=$//; s/^/-u /" | tr "\n" " ") \
  claude --bg --dangerously-skip-permissions \
  --model "$2" \
  "Read $1 and execute it exactly."' \
  workflow-worker {brief} {model} > {out} 2> {err}"#;

/// The value as one shell word, whatever is in it.
fn shq(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn subst(tpl: &str, key: &str, value: &str) -> String {
    tpl.replace(&format!("{{{key}}}"), &shq(value))
}

pub struct ClaudeBackend;

impl ClaudeBackend {
    fn template() -> String {
        match std::env::var("WORKFLOW_WORKER_CMD") {
            Ok(v) if !v.is_empty() => v,
            _ => WORKER_CMD_DEFAULT.to_string(),
        }
    }

    /// A custom template owns its own process shape (the test fakes write
    /// pidfiles and result documents), so it gets the legacy process
    /// semantics and never a `claude agents` call — which also keeps every
    /// fixture run hermetic on a machine with real sessions running.
    fn custom_template() -> bool {
        std::env::var("WORKFLOW_WORKER_CMD").is_ok_and(|v| !v.is_empty())
    }

    pub fn command_for(d: &Dispatch) -> String {
        let path = |p: &Path| p.to_string_lossy().to_string();
        let mut cmd = Self::template();
        for (key, value) in [
            ("worktree", path(&d.worktree)),
            ("brief", path(&d.brief)),
            ("out", path(&d.out)),
            ("err", path(&d.err)),
            ("pidfile", path(&d.pidfile)),
            ("status", path(&d.status)),
            ("task", d.task.clone()),
            ("rundir", path(&d.rundir)),
            ("session", d.session.clone()),
            ("model", d.model.clone()),
            ("turns", d.turns.clone()),
        ] {
            cmd = subst(&cmd, key, &value);
        }
        cmd
    }

    fn pid(h: &Handle) -> String {
        std::fs::read_to_string(&h.pidfile)
            .unwrap_or_default()
            .chars()
            .filter(|c| c.is_ascii_digit())
            .collect()
    }

    /// The session a just-finished dispatch actually started, as a full id.
    ///
    /// The minted id is not it: `--bg` ignores `--session-id` and chooses its
    /// own. Everything downstream reads this one -- the agents row, the
    /// transcript that answers for liveness, the `claude stop` that ends it --
    /// so getting it wrong is not a degraded run, it is a run where no worker
    /// is ever seen to finish and none can be stopped.
    ///
    /// Two ways to it, because the first is a printed line and printed lines
    /// change: the id `--bg` announced, resolved to its full form through the
    /// listing; failing that, the newest session standing in this worktree,
    /// which is the one that was just started there.
    fn adopt(d: &Dispatch) -> String {
        let printed = std::fs::read_to_string(&d.out).unwrap_or_default();
        let short = bg_id(&printed);
        let rows = agents_rows();

        if let Some(short) = short.as_deref()
            && let Some(row) = rows.iter().find(|r| r.id == short)
            && !row.session.is_empty()
        {
            return row.session.clone();
        }

        let wt = paths::realpath_m(&d.worktree);
        let mine = rows
            .iter()
            .filter(|r| !r.session.is_empty() && paths::realpath_m(&r.cwd) == wt)
            .max_by_key(|r| r.started);
        if let Some(row) = mine {
            return row.session.clone();
        }

        // Nothing to adopt. The short id is still a better handle than the
        // minted one -- `claude stop` takes it -- and the minted one is only
        // ever right for a template that was handed it.
        short.unwrap_or_else(|| d.session.clone())
    }
}

/// One row of `claude agents --json --all`. The listing is a union of two
/// shapes and neither field is guaranteed: every session carries `sessionId`,
/// `id` and `state`, and a session with a process still behind it carries
/// `pid` and `status` as well.
#[derive(Debug, Clone, Default, PartialEq)]
struct Row {
    id: String,
    session: String,
    cwd: String,
    state: String,
    status: String,
    started: i64,
}

impl Row {
    /// Is this session generating right now? `status` answers for a session
    /// with a process behind it; `state` is all a row keeps once the process
    /// is gone. Anything else -- idle, blocked, done, stopped -- is an ending,
    /// and the worker's status file says which kind.
    fn working(&self) -> bool {
        if !self.status.is_empty() {
            return self.status == "busy";
        }
        self.state == "working"
    }
}

/// Every row `claude agents --json --all` knows about -- `--all` because a
/// finished session leaves the live list but must still answer for its ending.
fn agents_rows() -> Vec<Row> {
    let Ok(out) = Command::new("claude")
        .args(["agents", "--json", "--all"])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    rows_in(&String::from_utf8_lossy(&out.stdout))
}

/// The pure half of `agents_rows`, so the parse is testable without a claude.
fn rows_in(json: &str) -> Vec<Row> {
    let Ok(serde_json::Value::Array(rows)) = serde_json::from_str(json) else {
        return Vec::new();
    };
    rows.iter()
        .map(|row| {
            let field = |k: &str| {
                row.get(k)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            };
            Row {
                id: field("id"),
                session: field("sessionId"),
                cwd: field("cwd"),
                state: field("state"),
                status: field("status"),
                started: row.get("startedAt").and_then(|v| v.as_i64()).unwrap_or(0),
            }
        })
        .collect()
}

/// The row for one handle. A handle is a full session id, but a dispatch whose
/// id could not be resolved falls back to the short one, so both are matched.
fn agents_row(session: &str) -> Option<Row> {
    if session.is_empty() {
        return None;
    }
    agents_rows()
        .into_iter()
        .find(|r| r.session == session || r.id == session)
}

/// Colour stripped, so a line printed for a terminal can be read as text.
fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        // CSI ... final byte in @-~; anything shorter just ends the escape.
        for e in chars.by_ref() {
            if e.is_ascii_alphabetic() || e == '~' || e == '@' {
                break;
            }
        }
    }
    out
}

/// What one session was carrying at its last turn, in tokens.
///
/// Every assistant line of a transcript records a `usage` object, and its input
/// side -- the fresh tokens, the ones written to cache and the ones read back
/// from it -- is the context that went into that turn. The last such line is
/// where the task ended up.
///
/// A worker that compacted mid-task reports what it carried afterwards, so the
/// number is a floor and not a high-water mark. That is enough for the one
/// question it answers: was this task cut too big?
fn last_context_tokens(transcript: &str) -> Option<u64> {
    let mut last = None;
    for line in transcript.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(usage) = v.get("message").and_then(|m| m.get("usage")) else {
            continue;
        };
        let field = |k: &str| usage.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
        last = Some(
            field("input_tokens")
                + field("cache_creation_input_tokens")
                + field("cache_read_input_tokens"),
        );
    }
    last
}

/// The id `claude --bg` printed on its way out. The line reads
/// `backgrounded · <id>`, coloured, and the id is the short form every other
/// `claude` verb takes.
fn bg_id(out: &str) -> Option<String> {
    let line = out.lines().find(|l| l.contains("backgrounded"))?;
    strip_ansi(line)
        .split_whitespace()
        .find(|w| w.len() == 8 && w.chars().all(|c| c.is_ascii_hexdigit()))
        .map(str::to_string)
}

impl WorkerBackend for ClaudeBackend {
    fn mint_session(&self) -> String {
        uuid::Uuid::new_v4().to_string()
    }

    fn dispatch(&self, d: &Dispatch) -> String {
        let mut c = Command::new("sh");
        c.arg("-c").arg(Self::command_for(d));
        for (k, v) in &d.env {
            c.env(k, v);
        }
        let _ = c.status();
        // A custom template was handed `{session}` and is the kind of thing
        // that honours it; only the shipped `--bg` line mints its own.
        if Self::custom_template() {
            return d.session.clone();
        }
        Self::adopt(d)
    }

    fn alive(&self, h: &Handle) -> bool {
        let pid = Self::pid(h);
        if !pid.is_empty() {
            return sys::pid_alive(&pid);
        }
        if Self::custom_template() {
            // Legacy process semantics: no pidfile means the dispatch never
            // got that far unless something with the session id still runs.
            if !h.session.is_empty() && sys::pgrep(&h.session) {
                return true;
            }
            return paths::transcript_path(&h.worktree, &h.session).exists();
        }
        match agents_row(&h.session) {
            // A resident session is alive while it works; idle or waiting is
            // an ending -- the status file says which kind.
            Some(row) => row.working(),
            // Not listed: either still launching (claim alive; the stall
            // deadline decides) or long gone with a transcript behind it.
            None => !paths::transcript_path(&h.worktree, &h.session).exists(),
        }
    }

    fn seen(&self, h: &Handle) -> bool {
        if !Self::pid(h).is_empty() {
            return true;
        }
        if paths::transcript_path(&h.worktree, &h.session).exists() {
            return true;
        }
        if Self::custom_template() {
            return !h.session.is_empty() && sys::pgrep(&h.session);
        }
        agents_row(&h.session).is_some()
    }

    fn last_activity(&self, h: &Handle) -> i64 {
        let transcript = sys::mtime(&paths::transcript_path(&h.worktree, &h.session));
        transcript.max(sys::newest_mtime(&h.worktree))
    }

    fn context_tokens(&self, h: &Handle) -> Option<u64> {
        let path = paths::transcript_path(&h.worktree, &h.session);
        last_context_tokens(&std::fs::read_to_string(path).ok()?)
    }

    fn stop(&self, h: &Handle, grace_s: i64) {
        let pid = Self::pid(h);
        if !pid.is_empty() {
            sys::kill_group(&pid, "TERM");
            let mut waited = 0;
            while waited < grace_s * 5 && sys::pid_alive(&pid) {
                sys::sleep(0.2);
                waited += 1;
            }
            sys::kill_group(&pid, "KILL");
            return;
        }
        // `claude stop` ends the session and keeps its conversation.
        if !Self::custom_template()
            && let Some(Row { id, .. }) = agents_row(&h.session)
            && !id.is_empty()
        {
            let _ = Command::new("claude")
                .args(["stop", &id])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            return;
        }
        if !h.session.is_empty() {
            sys::pkill(&h.session);
        }
    }

    fn result(&self, h: &Handle, out: &Path) -> Outcome {
        // Print-mode JSON first: the fakes and any custom template speak it.
        if let Ok(text) = std::fs::read_to_string(out)
            && !text.trim().is_empty()
            && let Ok(serde_json::Value::Object(v)) =
                serde_json::from_str::<serde_json::Value>(&text)
        {
            return Outcome {
                ok: !v.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false),
            };
        }
        // A --bg session leaves no result document; ending non-busy in the
        // agents list is the clean end, and the status file plus the merge
        // gate judge the work.
        if Self::custom_template() {
            return Outcome::default();
        }
        match agents_row(&h.session) {
            Some(row) => Outcome { ok: !row.working() },
            // Not listed at all. A transcript means it ran and the listing has
            // simply forgotten it -- that is an ending like any other, and the
            // status file and the merge gate judge the work. Neither means the
            // dispatch never happened, which is the one real error here.
            None => Outcome {
                ok: paths::transcript_path(&h.worktree, &h.session).exists(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            session: "018f2c7e-0000-4000-8000-000000000000".into(),
            model: "sonnet".into(),
            turns: "120".into(),
            env: Vec::new(),
        }
    }

    #[test]
    fn every_placeholder_is_substituted_as_one_shell_word() {
        let cmd = ClaudeBackend::command_for(&fixture());
        assert!(cmd.contains("cd '/state/my project/t1'"));
        assert!(cmd.contains("'/cache/briefs/t1.md'"));
        assert!(cmd.contains("> '/runs/t1.json' 2> '/runs/t1.err'"));
        assert!(!cmd.contains('{'), "a placeholder was left behind: {cmd}");
    }

    /// The two shapes the listing really serves: a session with a process
    /// behind it carries `pid`/`status`, and one without carries only `state`.
    const LISTING: &str = r#"[
        {"pid":1,"id":"aa11","cwd":"/wt/one","kind":"background","sessionId":"s-one",
         "state":"working","status":"busy","startedAt":100},
        {"pid":2,"id":"bb22","cwd":"/wt/two","kind":"background","sessionId":"s-two",
         "state":"working","status":"idle","startedAt":200},
        {"id":"cc33","cwd":"/wt/three","kind":"background","sessionId":"s-three",
         "state":"done","startedAt":300},
        {"id":"dd44","cwd":"/wt/four","kind":"background","sessionId":"s-four",
         "state":"blocked","startedAt":400}
    ]"#;

    fn row(session: &str) -> Row {
        rows_in(LISTING)
            .into_iter()
            .find(|r| r.session == session)
            .unwrap()
    }

    #[test]
    fn a_session_is_working_only_while_it_generates() {
        // A live process answers with `status`, and `busy` is the only value
        // that means the worker is still going.
        assert!(row("s-one").working());
        assert!(!row("s-two").working(), "resident but idle is an ending");
        // Once the process is gone there is only `state` to go on.
        assert!(!row("s-three").working());
        assert!(
            !row("s-four").working(),
            "waiting on a question is an ending"
        );
    }

    #[test]
    fn the_listing_parse_survives_the_fields_it_does_not_get() {
        let r = row("s-three");
        assert_eq!(r.id, "cc33");
        assert_eq!(r.cwd, "/wt/three");
        assert_eq!(r.started, 300);
        assert_eq!(r.status, "", "a row with no process has no status");
        assert!(rows_in("not json").is_empty());
        assert!(rows_in("{}").is_empty());
    }

    #[test]
    fn the_id_bg_prints_is_read_back_out_of_the_colour_it_prints_it_in() {
        let printed = "backgrounded · \u{1b}[36m66c356b2\u{1b}[39m\n  \
                       claude attach 66c356b2    open in this terminal\n";
        assert_eq!(bg_id(printed), Some("66c356b2".into()));
        // The minted uuid is not what comes back: --bg says so itself.
        assert_eq!(bg_id("warning: --bg manages the session id"), None);
        assert_eq!(bg_id(""), None);
    }

    #[test]
    fn a_quote_in_a_path_cannot_end_the_word() {
        let mut d = fixture();
        d.worktree = PathBuf::from("/state/it's here");
        let cmd = ClaudeBackend::command_for(&d);
        assert!(cmd.contains(r"cd '/state/it'\''s here'"));
    }

    #[test]
    fn the_scrub_and_the_flags_are_in_the_shipped_template() {
        for needle in [
            "claude --bg",
            "--dangerously-skip-permissions",
            "-u GITHUB_API_KEY",
            "-u WORKFLOW_ALLOW_PUSH",
            "-u WORKFLOW_HOOK_SEEN",
            "WORKFLOW_AGENT=1",
            "[A-Za-z0-9_]*(_TOKEN|_KEY|_SECRET)=",
        ] {
            assert!(
                WORKER_CMD_DEFAULT.contains(needle),
                "the template lost {needle}"
            );
        }
        // Print mode is banned for workers: sessions must be visible in the
        // agents view and attachable, and -p is neither.
        assert!(!WORKER_CMD_DEFAULT.contains(" -p "), "{WORKER_CMD_DEFAULT}");
        // And --session-id is not here: --bg mints its own id, warns that it
        // is ignoring the flag, and the handle is adopted from what it prints.
        assert!(
            !WORKER_CMD_DEFAULT.contains("--session-id"),
            "{WORKER_CMD_DEFAULT}"
        );
    }

    /// Two assistant turns and a user line between them. The answer is the
    /// last turn's whole input side, not a running total: what the worker was
    /// carrying when it stopped.
    const TRANSCRIPT: &str = r#"{"type":"user","message":{"role":"user"}}
{"type":"assistant","message":{"model":"claude-opus-5","usage":{"input_tokens":4,"cache_creation_input_tokens":900,"cache_read_input_tokens":12000,"output_tokens":50}}}
not json at all
{"type":"assistant","message":{"model":"claude-opus-5","usage":{"input_tokens":2,"cache_creation_input_tokens":1500,"cache_read_input_tokens":157000,"output_tokens":80}}}
{"type":"user","message":{"role":"user"}}
"#;

    #[test]
    fn the_context_a_worker_carried_is_the_last_turns_input_side() {
        assert_eq!(last_context_tokens(TRANSCRIPT), Some(2 + 1500 + 157000));
        // A usage object missing a field counts the ones it has.
        assert_eq!(
            last_context_tokens(r#"{"message":{"usage":{"input_tokens":7}}}"#),
            Some(7)
        );
        // Nothing to read is not zero: zero would say the worker used no
        // context, and the honest answer is that the backend cannot see.
        assert_eq!(last_context_tokens(""), None);
        assert_eq!(last_context_tokens("{\"type\":\"user\"}\n"), None);
    }

    #[test]
    fn a_session_id_is_a_uuid_v4() {
        let s = ClaudeBackend.mint_session();
        let parts: Vec<&str> = s.split('-').collect();
        assert_eq!(
            parts.iter().map(|p| p.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12]
        );
        assert!(s.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
        assert!(parts[2].starts_with('4'));
        assert!(matches!(&parts[3][0..1], "8" | "9" | "a" | "b"));
        assert_ne!(s, ClaudeBackend.mint_session());
    }
}
