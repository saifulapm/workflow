//! The worker backend seam (spec §12b).
//!
//! amx is the execution and visibility substrate, not an orchestrator: `run`
//! dispatches *onto* a backend and keeps every policy decision -- waves,
//! ownership, the merge gate, park -- to itself. The `claude` backend below
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
    pub budget: String,
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
    pub cost: f64,
}

pub trait WorkerBackend {
    /// A fresh handle for one dispatch. A redispatch mints a new one.
    fn mint_session(&self) -> String;
    /// Start the worker. Detached: this returns as soon as it is going.
    fn dispatch(&self, d: &Dispatch);
    /// Is anything of this worker still running?
    fn alive(&self, h: &Handle) -> bool;
    /// The most recent sign of life the backend can see, in epoch seconds.
    /// The worker's own status file is the orchestrator's signal, not the
    /// backend's, and is counted on top of this.
    fn last_activity(&self, h: &Handle) -> i64;
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
/// service and returns. The session id we mint is the whole handle -- the
/// agents list answers for it from then on. `--max-budget-usd` rides along
/// but its enforcement outside print mode is unverified; the deadline, the
/// worker cap and one-task briefs are the bounds that are known to hold.
pub const WORKER_CMD_DEFAULT: &str = r#"cd {worktree} && WORKFLOW_AGENT=1 sh -c '\
  exec env -u GITHUB_API_KEY -u WORKFLOW_ALLOW_PUSH -u WORKFLOW_HOOK_SEEN \
  $(env | grep -oE "^[A-Za-z0-9_]*(_TOKEN|_KEY|_SECRET)=|^(GH_|GITHUB_|AWS_|STRIPE_)[A-Za-z0-9_]*=" | sed "s/=$//; s/^/-u /" | tr "\n" " ") \
  claude --bg --dangerously-skip-permissions --max-budget-usd "$2" \
  --model "$3" --session-id "$4" \
  "Read $1 and execute it exactly."' \
  workflow-worker {brief} {budget} {model} {session} > {out} 2> {err}"#;

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
            ("budget", d.budget.clone()),
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
}

/// The agents-list row for one session: (short id, status). Shells out to
/// `claude agents --json --all` -- `--all` because a finished session leaves
/// the live list but must still answer for its ending.
fn agents_row(session: &str) -> Option<(String, String)> {
    let out = Command::new("claude")
        .args(["agents", "--json", "--all"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    row_in(&String::from_utf8_lossy(&out.stdout), session)
}

/// The pure half of `agents_row`, so the parse is testable without a claude.
fn row_in(json: &str, session: &str) -> Option<(String, String)> {
    let rows: serde_json::Value = serde_json::from_str(json).ok()?;
    for row in rows.as_array()? {
        if row.get("sessionId").and_then(|s| s.as_str()) == Some(session) {
            let field = |k: &str| {
                row.get(k)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            };
            return Some((field("id"), field("status")));
        }
    }
    None
}

impl WorkerBackend for ClaudeBackend {
    fn mint_session(&self) -> String {
        uuid::Uuid::new_v4().to_string()
    }

    fn dispatch(&self, d: &Dispatch) {
        let mut c = Command::new("sh");
        c.arg("-c").arg(Self::command_for(d));
        for (k, v) in &d.env {
            c.env(k, v);
        }
        let _ = c.status();
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
            Some((_, status)) => status == "busy",
            // Not listed: either still launching (claim alive; the stall
            // deadline decides) or long gone with a transcript behind it.
            None => !paths::transcript_path(&h.worktree, &h.session).exists(),
        }
    }

    fn last_activity(&self, h: &Handle) -> i64 {
        let transcript = sys::mtime(&paths::transcript_path(&h.worktree, &h.session));
        transcript.max(sys::newest_mtime(&h.worktree))
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
            && let Some((id, _)) = agents_row(&h.session)
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
                cost: v
                    .get("total_cost_usd")
                    .and_then(|c| c.as_f64())
                    .unwrap_or(0.0),
            };
        }
        // A --bg session leaves no result document; ending non-busy in the
        // agents list is the clean end, and the status file plus the merge
        // gate judge the work. Cost is not knowable here, so the run budget
        // breaker is inert on this backend -- the deadline, the worker cap
        // and one-task briefs are the bounds.
        if Self::custom_template() {
            return Outcome::default();
        }
        match agents_row(&h.session) {
            Some((_, status)) => Outcome {
                ok: status != "busy",
                cost: 0.0,
            },
            None => Outcome::default(),
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
            budget: "10".into(),
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

    #[test]
    fn the_agents_row_parse_finds_a_session_and_only_that_session() {
        let json = r#"[
            {"pid":1,"id":"aa11","kind":"background","sessionId":"s-one","status":"busy"},
            {"pid":2,"id":"bb22","kind":"background","sessionId":"s-two","status":"idle"}
        ]"#;
        assert_eq!(row_in(json, "s-one"), Some(("aa11".into(), "busy".into())));
        assert_eq!(row_in(json, "s-two"), Some(("bb22".into(), "idle".into())));
        assert_eq!(row_in(json, "s-three"), None);
        assert_eq!(row_in("not json", "s-one"), None);
        assert_eq!(row_in("{}", "s-one"), None);
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
            "--max-budget-usd",
            "--session-id",
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
