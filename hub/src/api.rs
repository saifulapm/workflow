//! `GET /api/presence`: whether someone is at this machine, for a sibling
//! hub or a session deciding where a question should go.

/// `/api/presence` — the doorbell-routing answer for this machine, for a
/// sibling hub deciding where the bell should ring.
pub fn presence(p: &crate::presence::Presence, machine: &str) -> String {
    serde_json::json!({
        "machine": machine,
        "watching": p.watching(),
        "shell": p.shell,
        "locked": p.locked,
        "stay_awake": p.stay_awake,
        "idle": p.idle,
    })
    .to_string()
}
