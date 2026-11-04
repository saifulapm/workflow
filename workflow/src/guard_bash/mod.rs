//! `workflow guard bash`, a Claude Code PreToolUse hook for every session. It
//! refuses a Bash command that repeats a lesson's mistake, and says the safe
//! form in two lines at most. A Monitor call's command reaches the hook as a
//! Bash call too. Input it cannot read passes: a broken guard must not stop
//! the agent.

mod chain;

use std::io::Read;
use std::path::Path;

use serde_json::Value;

use crate::exit;
use crate::shell::{self, Simple};

/// Claude Code blocks a tool call whose PreToolUse hook exits 2, and shows
/// the agent what the hook wrote to stderr.
const BLOCK: i32 = 2;

/// A command line and the folder it runs in.
pub struct Call<'a> {
    pub commands: &'a [Simple],
    pub cwd: &'a Path,
}

/// Each rule's refusal for a call, or None. The first refusal is the one
/// the agent reads.
const RULES: &[fn(&Call) -> Option<&'static str>] = &[chain::refusal];

pub fn cmd_guard_bash() -> i32 {
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        return exit::OK;
    }
    let Ok(hook) = serde_json::from_str::<Value>(&input) else {
        return exit::OK;
    };
    if hook["tool_name"] != "Bash" {
        return exit::OK;
    }
    let Some(command) = hook["tool_input"]["command"].as_str() else {
        return exit::OK;
    };
    let Some(commands) = shell::commands(command) else {
        return exit::OK;
    };
    let call = Call {
        commands: &commands,
        cwd: Path::new(hook["cwd"].as_str().unwrap_or_default()),
    };
    match RULES.iter().find_map(|rule| rule(&call)) {
        Some(reason) => {
            eprintln!("{reason}");
            BLOCK
        }
        None => exit::OK,
    }
}
