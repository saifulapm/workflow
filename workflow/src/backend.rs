//! The worker backend seam (spec §12b).
//!
//! amx is the execution and visibility substrate, not an orchestrator: `run`
//! dispatches *onto* a backend and keeps every policy decision -- waves,
//! ownership, the merge gate, park -- to itself. The `claude` backend below is
//! today's detached print-mode worker: a shell template, a pidfile, a session
//! id that `--session-id` insists is a UUID, and a transcript to watch. An
//! `amx` backend maps the same four questions onto `amx new` / `amx ls` /
//! `amx result` / `amx stop` and deletes the pidfile and group-kill machinery
//! with nothing above it changing.

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
    /// What the worker left behind.
    fn result(&self, out: &Path) -> Outcome;
}

/// The dispatch template (spec §8.3). Every placeholder sits where a single
/// shell-quoted word is legal -- at the outer level, or as a positional argument
/// to the inner `sh -c` -- so a project whose path has a space in it dispatches
/// like any other. A placeholder nested inside the inner script's own quoting
/// could not be quoted correctly for both shells at once.
///
/// The `env -u` sweep is the credential scrub of spec §1. The two workflow
/// variables go with it: an orchestrator run under WORKFLOW_ALLOW_PUSH would
/// otherwise release the pre-push refusal for every worker it dispatches, and an
/// inherited WORKFLOW_HOOK_SEEN would tell a worker's first commit that the gate
/// had already run.
pub const WORKER_CMD_DEFAULT: &str = r#"cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c 'echo $$ > "$1"; \
  exec env -u GITHUB_API_KEY -u WORKFLOW_ALLOW_PUSH -u WORKFLOW_HOOK_SEEN \
  $(env | grep -oE "^[A-Za-z0-9_]*(_TOKEN|_KEY|_SECRET)=|^(GH_|GITHUB_|AWS_|STRIPE_)[A-Za-z0-9_]*=" | sed "s/=$//; s/^/-u /" | tr "\n" " ") \
  claude -p --output-format json \
  --dangerously-skip-permissions --max-budget-usd "$3" --max-turns "$4" \
  --model "$5" --session-id "$6" \
  "Read $2 and execute it exactly."' \
  workflow-worker {pidfile} {brief} {budget} {turns} {model} {session} > {out} 2> {err} &"#;

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
        // No pidfile yet: the session id is the only handle we have, and if the
        // transcript is missing too there is nothing alive to wait for.
        if !h.session.is_empty() && sys::pgrep(&h.session) {
            return true;
        }
        paths::transcript_path(&h.worktree, &h.session).exists()
    }

    fn last_activity(&self, h: &Handle) -> i64 {
        let transcript = sys::mtime(&paths::transcript_path(&h.worktree, &h.session));
        transcript.max(sys::newest_mtime(&h.worktree))
    }

    fn stop(&self, h: &Handle, grace_s: i64) {
        let pid = Self::pid(h);
        if pid.is_empty() {
            if !h.session.is_empty() {
                sys::pkill(&h.session);
            }
            return;
        }
        sys::kill_group(&pid, "TERM");
        let mut waited = 0;
        while waited < grace_s * 5 && sys::pid_alive(&pid) {
            sys::sleep(0.2);
            waited += 1;
        }
        sys::kill_group(&pid, "KILL");
    }

    fn result(&self, out: &Path) -> Outcome {
        let Ok(text) = std::fs::read_to_string(out) else {
            return Outcome::default();
        };
        if text.is_empty() {
            return Outcome::default();
        }
        let Ok(serde_json::Value::Object(v)) = serde_json::from_str::<serde_json::Value>(&text)
        else {
            // Malformed, or not an object: the worker did not report cleanly.
            return Outcome::default();
        };
        Outcome {
            ok: !v.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false),
            cost: v
                .get("total_cost_usd")
                .and_then(|c| c.as_f64())
                .unwrap_or(0.0),
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
        assert!(cmd.contains("'/runs/t1.pid'"));
        assert!(cmd.contains("> '/runs/t1.json' 2> '/runs/t1.err'"));
        assert!(!cmd.contains('{'), "a placeholder was left behind: {cmd}");
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
            "-p --output-format json",
            "--dangerously-skip-permissions",
            "--max-budget-usd",
            "--max-turns",
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
