//! The worker backend seam (spec §12b).
//!
//! amx is the execution and visibility substrate, not an orchestrator: `run`
//! dispatches *onto* a backend and keeps every policy decision -- the ready
//! set, ownership, the merge gate -- to itself. The backend that ships is
//! [`crate::backend_amx::AmxBackend`]: every worker and every reader is an
//! amx agent, visible in `amx ls`, attachable, ended with `amx stop`,
//! answered for by `amx status --json`. The [`ProcessBackend`] below is the
//! test seam and nothing else: a `WORKFLOW_WORKER_CMD` template that owns
//! its own process shape (pidfile, signals, a print-mode result document),
//! which is how the suite fakes a worker without a tmux server.

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
    /// The reasoning dial, when the run has one to pass: `--effort` on both
    /// backends. `None` adds no flag, and the CLI's own default stands.
    pub effort: Option<String>,
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
    /// Detached: this returns as soon as it is going. The handle a backend
    /// hands back after launch is the authoritative one and is what gets
    /// recorded; empty means the launch was refused and nothing runs, with
    /// the refusal on `Dispatch::err`.
    fn dispatch(&self, d: &Dispatch) -> String;
    /// Is anything of this worker still running?
    fn alive(&self, h: &Handle) -> bool;
    /// Does the backend hold any record of this session at all -- a listing
    /// row, a process carrying its id, a transcript, a pidfile? Distinct from
    /// `alive`: a session can be seen and ended. At dispatch time no record
    /// means still launching; at adoption, with the run that recorded the
    /// session dead, it means the session never existed (friction #9F7WT13K).
    fn seen(&self, h: &Handle) -> bool;
    /// Is the session standing right now -- a pane amx can still read, a
    /// live pid? Narrower than `seen`: a record of a pane that is gone is a
    /// record, not a listing. This is what tells a session the usage limit
    /// paused (a pane up and idle) from one that died with the machine (a
    /// record and nothing else), which `seen` cannot (frictions #B3391C6H,
    /// #QT1PDNRK).
    fn listed(&self, h: &Handle) -> bool;
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
    /// The transcript's last text -- what the worker said on its last turn,
    /// read off its own conversation file. Empty when there is nothing to
    /// read there, which for a custom template is every time: it never
    /// touches `~/.claude/projects`.
    fn last_words(&self, _h: &Handle) -> String {
        String::new()
    }
}

/// `Dispatch.env` as the JSON object `--settings` takes. `env` is the key
/// the flag reads for the session's own environment, so this is how a
/// per-task value -- the task's own tag, its own cargo target dir -- reaches
/// the worker's process rather than just the shell dispatch runs it under.
pub fn settings_json(env: &[(String, String)]) -> String {
    let mut vars = serde_json::Map::new();
    for (k, v) in env {
        vars.insert(k.clone(), serde_json::Value::String(v.clone()));
    }
    let mut settings = serde_json::Map::new();
    settings.insert("env".to_string(), serde_json::Value::Object(vars));
    serde_json::Value::Object(settings).to_string()
}

/// The value as one shell word, whatever is in it.
fn shq(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn subst(tpl: &str, key: &str, value: &str) -> String {
    tpl.replace(&format!("{{{key}}}"), &shq(value))
}

/// The test seam: a `WORKFLOW_WORKER_CMD` template evaluated by `sh -c`,
/// with legacy process semantics -- the template owns `cd`, `setsid`, the
/// pidfile and the redirections, and group-kill depends on it. Every
/// placeholder sits where a single shell-quoted word is legal, so a project
/// whose path has a space in it dispatches like any other.
pub struct ProcessBackend;

impl ProcessBackend {
    /// Whether a caller asked for this seam at all. Empty is unset: the
    /// suite's lib.sh clears the variable, it does not set it to nothing.
    pub fn wanted() -> bool {
        std::env::var("WORKFLOW_WORKER_CMD").is_ok_and(|v| !v.is_empty())
    }

    pub fn command_for(d: &Dispatch) -> String {
        Self::substitute(&std::env::var("WORKFLOW_WORKER_CMD").unwrap_or_default(), d)
    }

    fn substitute(tpl: &str, d: &Dispatch) -> String {
        let path = |p: &Path| p.to_string_lossy().to_string();
        let mut cmd = tpl.to_string();
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
            ("effort", d.effort.clone().unwrap_or_default()),
            ("turns", d.turns.clone()),
            ("settings", settings_json(&d.env)),
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

pub(crate) fn last_context_tokens(transcript: &str) -> Option<u64> {
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

/// The joined text of a transcript's last turn that said anything -- a tool
/// call carries no text and leaves the turn before it standing, the way a
/// worker that ended mid-thought does not overwrite what it last actually
/// said. A user turn is never it, even one carrying text: a reading that
/// ends before the assistant speaks has said nothing, and marker text in a
/// user turn -- CLAUDE.md, a system reminder -- is not the worker's own word.
///
/// A worker under amx is a claude session in a pane and writes this
/// transcript; the session id naming the file is amx's to give.
pub(crate) fn last_words_in(transcript: &str) -> String {
    let mut last = String::new();
    for line in transcript.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(message) = v.get("message") else {
            continue;
        };
        if message.get("role").and_then(|r| r.as_str()) != Some("assistant") {
            continue;
        }
        let Some(content) = message.get("content") else {
            continue;
        };
        let text = match content {
            serde_json::Value::String(s) => s.trim().to_string(),
            serde_json::Value::Array(items) => items
                .iter()
                .filter_map(|b| {
                    (b.get("type").and_then(|t| t.as_str()) == Some("text"))
                        .then(|| b.get("text").and_then(|t| t.as_str()))
                        .flatten()
                })
                .collect::<Vec<_>>()
                .join("")
                .trim()
                .to_string(),
            _ => String::new(),
        };
        if !text.is_empty() {
            last = text;
        }
    }
    last
}

impl WorkerBackend for ProcessBackend {
    fn mint_session(&self) -> String {
        uuid::Uuid::new_v4().to_string()
    }

    /// The template was handed `{session}` and honours it, so the minted id
    /// is the handle.
    fn dispatch(&self, d: &Dispatch) -> String {
        let mut c = Command::new("sh");
        c.arg("-c").arg(Self::command_for(d));
        for (k, v) in &d.env {
            c.env(k, v);
        }
        let _ = c.status();
        d.session.clone()
    }

    /// The pidfile decides; without one, something carrying the session id
    /// still runs, or the dispatch never got that far and a transcript is
    /// the only sign it ever started.
    fn alive(&self, h: &Handle) -> bool {
        let pid = Self::pid(h);
        if !pid.is_empty() {
            return sys::pid_alive(&pid);
        }
        if !h.session.is_empty() && sys::pgrep(&h.session) {
            return true;
        }
        paths::transcript_path(&h.worktree, &h.session).exists()
    }

    fn seen(&self, h: &Handle) -> bool {
        !Self::pid(h).is_empty()
            || paths::transcript_path(&h.worktree, &h.session).exists()
            || (!h.session.is_empty() && sys::pgrep(&h.session))
    }

    /// A process is either running or it is not; a dead one is dead, not
    /// idle, so this never says "listed and paused".
    fn listed(&self, h: &Handle) -> bool {
        let pid = Self::pid(h);
        if !pid.is_empty() {
            return sys::pid_alive(&pid);
        }
        !h.session.is_empty() && sys::pgrep(&h.session)
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
        if !h.session.is_empty() {
            sys::pkill(&h.session);
        }
    }

    /// The print-mode result document the fakes write; nothing there is a
    /// worker that ended without one, and the status file and the merge gate
    /// judge the work.
    fn result(&self, _h: &Handle, out: &Path) -> Outcome {
        if let Ok(text) = std::fs::read_to_string(out)
            && !text.trim().is_empty()
            && let Ok(serde_json::Value::Object(v)) =
                serde_json::from_str::<serde_json::Value>(&text)
        {
            return Outcome {
                ok: !v.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false),
            };
        }
        Outcome::default()
    }

    fn last_words(&self, h: &Handle) -> String {
        let path = paths::transcript_path(&h.worktree, &h.session);
        last_words_in(&std::fs::read_to_string(path).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TPL: &str = "cd {worktree} && exec worker {task} {brief} {out} {err} {pidfile} {status} {rundir} {session} {model} {effort} {turns} {settings} > {out} 2> {err}";

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
            effort: None,
            turns: "120".into(),
            env: Vec::new(),
        }
    }

    #[test]
    fn every_placeholder_is_substituted_as_one_shell_word() {
        let cmd = ProcessBackend::substitute(TPL, &fixture());
        assert!(cmd.contains("cd '/state/my project/t1'"));
        assert!(cmd.contains("'/cache/briefs/t1.md'"));
        assert!(cmd.contains("> '/runs/t1.json' 2> '/runs/t1.err'"));
        for key in [
            "worktree", "brief", "out", "err", "pidfile", "status", "task", "rundir", "session",
            "model", "effort", "turns", "settings",
        ] {
            let placeholder = format!("{{{key}}}");
            assert!(
                !cmd.contains(&placeholder),
                "{placeholder} was left behind: {cmd}"
            );
        }
    }

    #[test]
    fn the_settings_json_holds_every_env_pair() {
        let json = settings_json(&[
            ("CARGO_TARGET_DIR".into(), "/state/cargo/t1".into()),
            ("WORKFLOW_TASK".into(), "env-carry/t1".into()),
        ]);
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        assert_eq!(v["env"]["CARGO_TARGET_DIR"], "/state/cargo/t1");
        assert_eq!(v["env"]["WORKFLOW_TASK"], "env-carry/t1");
    }

    /// An unset dial is an empty word, never `--effort ''`: the template
    /// decides what to do with nothing, as the fakes do with `${7:+...}`.
    #[test]
    fn the_effort_level_is_its_own_word_and_empty_when_unset() {
        let mut d = fixture();
        assert!(ProcessBackend::substitute(TPL, &d).contains("'sonnet' '' '120'"));
        d.effort = Some("max".into());
        assert!(ProcessBackend::substitute(TPL, &d).contains("'sonnet' 'max' '120'"));
    }

    #[test]
    fn a_template_without_the_settings_placeholder_is_unchanged() {
        assert_eq!(
            subst("no placeholder here", "settings", "{\"env\":{}}"),
            "no placeholder here"
        );
    }

    #[test]
    fn a_quote_in_a_path_cannot_end_the_word() {
        let mut d = fixture();
        d.worktree = PathBuf::from("/state/it's here");
        let cmd = ProcessBackend::substitute(TPL, &d);
        assert!(cmd.contains(r"cd '/state/it'\''s here'"));
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

    /// A user turn, an assistant turn with a tool call and no text, and a
    /// final assistant turn with two text blocks. The answer is the joined
    /// text of that last turn, not the tool call and not the user line.
    const WORDS_TRANSCRIPT: &str = r#"{"type":"user","message":{"role":"user","content":"go"}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","name":"Bash"}]}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"You've reached your "},{"type":"text","text":"Fable limit."}]}}
"#;

    #[test]
    fn the_last_words_are_the_last_turns_text_joined() {
        assert_eq!(
            last_words_in(WORDS_TRANSCRIPT),
            "You've reached your Fable limit."
        );
        // A turn with no text block leaves the last text before it standing.
        assert_eq!(
            last_words_in(&format!(
                "{WORDS_TRANSCRIPT}{{\"type\":\"assistant\",\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"tool_use\",\"name\":\"Bash\"}}]}}}}\n"
            )),
            "You've reached your Fable limit."
        );
        assert_eq!(last_words_in(""), "");
        assert_eq!(last_words_in("not json at all"), "");
    }

    #[test]
    fn a_users_turn_is_never_the_last_words() {
        // A reading that ends before the assistant answers leaves only a
        // user turn -- the dispatch's own prompt, or a rate-limit mention
        // planted in it -- which must never stand in as the reader's own.
        assert_eq!(
            last_words_in(
                r#"{"type":"user","message":{"role":"user","content":"reviewing t2, mind the rate limit"}}"#
            ),
            ""
        );
    }

    #[test]
    fn a_session_id_is_a_uuid_v4() {
        let s = ProcessBackend.mint_session();
        let parts: Vec<&str> = s.split('-').collect();
        assert_eq!(
            parts.iter().map(|p| p.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12]
        );
        assert!(s.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
        assert!(parts[2].starts_with('4'));
        assert!(matches!(&parts[3][0..1], "8" | "9" | "a" | "b"));
        assert_ne!(s, ProcessBackend.mint_session());
    }
}
