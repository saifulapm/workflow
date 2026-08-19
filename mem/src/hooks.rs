//! Runtime hook modes (spec §9).
//!
//! Three channels, and they are not interchangeable — each was checked against
//! the Claude Code 2.1.233 binary:
//!
//! - **PostToolBatch** and **Stop** read `hookSpecificOutput.additionalContext`.
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

/// Batches between two injections of the brief (spec §9).
pub const BATCH_EVERY: u64 = 5;

/// The Stop nudge: ~30 tokens, and both halves are actionable.
pub const NUDGE: &str = "nothing recorded this session — call `mem log`/`mem save` if anything \
     is worth keeping; write `mem handoff` now if the work is unfinished";

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

/// The PostToolBatch half of `mem context --brief`: count this batch, and emit
/// the brief on every fifth one. The counter lives in the machine-local session
/// file because the hook input carries no batch index.
///
/// Without a session id there is nowhere to count, so every batch emits — the
/// wiring in `mem/TESTING.md` always passes one.
pub fn post_tool_batch(app: &App, brief: &str) -> Result<i32> {
    if let Some(session) = &app.session_id {
        let n = crate::session::record_batch(&app.dirs.sessions_dir(), session);
        if !n.is_multiple_of(BATCH_EVERY) {
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

/// `mem session-check` — the Stop hook. A session that recorded nothing gets one
/// nudge; a session that wrote gets silence.
pub fn session_check(app: &App, hook_json: bool) -> Result<i32> {
    // No project resolution: session activity is machine-local and the answer
    // does not depend on where the session ran, so the Stop hook does not pay
    // for a git call it cannot use.
    let activity = match &app.session_id {
        Some(session) => crate::session::read(&app.dirs.sessions_dir(), session),
        None => crate::session::Activity::default(),
    };
    let nudge = if activity.writes == 0 {
        Some(NUDGE)
    } else {
        None
    };

    if app.json {
        println!(
            "{}",
            serde_json::to_string(&json!({
                "writes": activity.writes,
                "batches": activity.batches,
                "nudge": nudge,
                "nudged": activity.nudged,
            }))?
        );
        return Ok(exit::OK);
    }

    // The diagnostic view above answers "did this session record anything".
    // What gets *emitted* is a narrower question, because the runtime replies
    // to a Stop hook's additionalContext by waking the model, which answers and
    // stops again: the nudge fires once per session, and only for a session
    // whose id makes "once" a thing mem can count.
    let Some(session) = app.session_id.as_deref() else {
        return Ok(exit::OK);
    };
    if let Some(nudge) = nudge {
        if activity.nudged {
            return Ok(exit::OK);
        }
        crate::session::record_nudge(&app.dirs.sessions_dir(), session);
        if hook_json {
            println!("{}", serde_json::to_string(&envelope("Stop", nudge))?);
        } else {
            println!("{nudge}");
        }
    }
    Ok(exit::OK)
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
