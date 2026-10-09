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
