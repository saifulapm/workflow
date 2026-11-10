//! `workflow guard bash`, a Claude Code PreToolUse hook for every session. It
//! refuses a Bash command that repeats a lesson's mistake, and says the safe
//! form in two lines at most. A Monitor call's command reaches the hook as a
//! Bash call too. Input it cannot read passes: a broken guard must not stop
//! the agent.

mod attach;
mod cd;
mod chain;
mod devserver;
mod pkill;
mod playwright;
mod tmp;

use std::io::Read;
use std::path::Path;

use serde_json::Value;

use crate::exit;
use crate::shell::{self, Simple};

/// Claude Code blocks a tool call whose PreToolUse hook exits 2. With a deny
/// decision on stdout it shows the agent that decision's reason; one that
/// cannot read the decision shows stderr, so both carry the reason. Every
/// form of refusal reads "PreToolUse:Bash hook error: " in Claude Code
/// 2.1.318, but the decision drops the hook's own command from in front of
/// the reason. An exit 0 with a decision it rejected would run the call.
const BLOCK: i32 = 2;

/// A command line and the folder it runs in.
pub struct Call<'a> {
    pub commands: &'a [Simple],
    pub cwd: &'a Path,
    /// The subagent the call runs in, as the hook's `agent_type` names it;
    /// None in the session's own agent.
    pub agent: Option<&'a str>,
}

/// Each rule's refusal for a call, or None. The first refusal is the one
/// the agent reads.
const RULES: &[fn(&Call) -> Option<&'static str>] = &[
    chain::refusal,
    pkill::refusal,
    attach::refusal,
    playwright::refusal,
    cd::refusal,
    tmp::refusal,
    devserver::refusal,
];

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
        agent: hook["agent_type"].as_str(),
    };
    match RULES.iter().find_map(|rule| rule(&call)) {
        Some(reason) => {
            let decision = serde_json::json!({
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "deny",
                    "permissionDecisionReason": reason,
                }
            });
            println!("{decision}");
            eprintln!("{reason}");
            BLOCK
        }
        None => exit::OK,
    }
}
