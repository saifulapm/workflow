//! Runtime hook modes.
//!
//! Two channels, and they are not interchangeable — each was checked against
//! the Claude Code 2.1.233 binary:
//!
//! - **PostToolBatch** reads `hookSpecificOutput.additionalContext`.
//!   The event name in the envelope must be the event that ran; the binary
//!   errors on a mismatch and validates the envelope against a schema, so an
//!   unknown key is dropped and an unknown `hookEventName` fails the whole
//!   output.
//! - **PreCompact** has no `hookSpecificOutput` variant at all. Its channel is
//!   the hook's plain stdout, which the runtime hands to the summarizer as
//!   `newCustomInstructions`. Emitting JSON there would fail schema validation
//!   and be discarded silently, so `mem precompact` prints a sentence.
//!
//! - **Stop** reads `{"decision":"block","reason":"..."}` on stdout and keeps
//!   the session going with the reason; printing nothing lets it stop.
//!
//! Everything here exits 0. A hook that fails is noise in someone's session,
//! and none of this is important enough to interrupt a session over.

use anyhow::Result;
use serde_json::json;

use crate::app::App;
use crate::exit;

/// What `mem precompact` hands the summarizer.
pub const PRECOMPACT: &str =
    "Preserve the current mem handoff and the exact next action verbatim in the summary.";

/// `{"hookSpecificOutput":{"hookEventName":"<event>","additionalContext":"<text>"}}`
pub fn envelope(event: &str, context: &str) -> serde_json::Value {
    json!({
        "hookSpecificOutput": {
            "hookEventName": event,
            "additionalContext": context,
        }
    })
}

/// The PostToolBatch half of `mem context --brief`: emit the brief only when it
/// differs from the one this session last saw. The session start's digest
/// already carried the first, so the first batch only records it. The last
/// brief lives in the machine-local session file because the hook input
/// carries none.
///
/// Without a session id there is nothing to compare with, so every batch
/// emits — the hook wiring always passes one.
pub fn post_tool_batch(app: &App, brief: &str) -> Result<i32> {
    if let Some(session) = &app.session_id {
        let settled = settled(brief);
        let seen = crate::session::record_batch(&app.dirs.sessions_dir(), session, &settled);
        if seen.is_none_or(|seen| seen == settled) {
            return Ok(exit::OK);
        }
    }
    // An empty brief is not worth an injection.
    if !brief.is_empty() {
        println!(
            "{}",
            serde_json::to_string(&envelope("PostToolBatch", brief))?
        );
    }
    Ok(exit::OK)
}

/// The brief as two batches compare it. While the sync unit is behind, the
/// warning's age goes up every minute, and a number ticking up is not news.
fn settled(brief: &str) -> String {
    brief
        .lines()
        .map(|line| {
            if line.starts_with(crate::sync::LAST_SYNCED) {
                crate::sync::LAST_SYNCED
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `mem precompact` — plain text, on purpose (see the module note).
pub fn precompact(app: &App) -> Result<i32> {
    if app.json {
        println!(
            "{}",
            serde_json::to_string(&json!({ "newCustomInstructions": PRECOMPACT }))?
        );
    } else {
        println!("{PRECOMPACT}");
    }
    Ok(exit::OK)
}

/// `mem stop-hook --session-id <id>`: a cloud session's VM goes offline when
/// its turn ends, and the answer to its question, sent peer to peer to that
/// VM's hub, is lost. So the stop is blocked while a question this session
/// asked is still open on this machine's hub, the one an answer reaches.
/// The hub, not the store, says what is open: over the remote mem server
/// the store never hears of an answer the hub took.
///
/// It lets the stop through when this session asked nothing still open, when
/// the latest handoff, written after its questions, starts `parked:` (the
/// 6-hour stop), and when there is no hub to ask. It does not let a second
/// stop through on `stop_hook_active`, as the hooks guide suggests: each
/// wait is a tool call, which resets Claude Code's cap of eight blocks in a
/// row, so the session waits for as long as the question is open.
pub fn stop(app: &App) -> Result<i32> {
    let Some(session) = &app.session_id else {
        return Ok(exit::OK);
    };
    let asked = crate::session::read(&app.dirs.sessions_dir(), session).asked;
    if asked.is_empty() {
        return Ok(exit::OK);
    }
    let socket = crate::localhub::socket_path(&app.dirs.state);
    let Some(open_here) = crate::localhub::open_here(&socket) else {
        return Ok(exit::OK);
    };
    let open: Vec<&String> = asked
        .iter()
        .filter(|id| open_here.contains(&crate::ids::short_id(id)))
        .collect();
    if open.is_empty() {
        return Ok(exit::OK);
    }
    let identity = app.identity(crate::project::Mode::Read)?;
    let index = app.read_index()?;
    if let Some(handoff) = index.recent("handoff", identity.id(), 1)?.into_iter().next()
        && crate::verbs::read_body(&handoff).trim_start().starts_with("parked:")
        // ULIDs sort by the time they were made.
        && open.iter().all(|id| handoff.id.as_str() > id.as_str())
    {
        return Ok(exit::OK);
    }
    let shorts: Vec<String> = open.iter().map(|id| crate::ids::short_id(id)).collect();
    let named: Vec<String> = shorts.iter().map(|s| format!("#{s}")).collect();
    let waits: Vec<String> = shorts
        .iter()
        .map(|s| format!("`mem questions --wait {s}`"))
        .collect();
    let (noun, verb) = if named.len() > 1 {
        ("questions", "are")
    } else {
        ("question", "is")
    };
    // The reason names no way out but the 6-hour stop, so a session that
    // tires of waiting does not park on the question.
    let reason = format!(
        "Your {noun} {} {verb} still unanswered. Do not end the turn: this cloud \
         session's VM goes offline when it ends, and the answer could not reach it. \
         Run {}, and run it again each time it returns without an answer. Do not \
         park on it: only the 6-hour stop ends the turn with a question open.",
        named.join(", "),
        waits.join(", then ")
    );
    println!(
        "{}",
        serde_json::to_string(&json!({ "decision": "block", "reason": reason }))?
    );
    Ok(exit::OK)
}
