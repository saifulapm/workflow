//! `GET /api/questions | /api/activity | /api/projects` (spec §3).
//!
//! The same data the page shows, in the shapes committed under `hub/schemas/`.
//! Each envelope is closed and carries exactly two keys: the rows, and why the
//! section is degraded — which is `null` for an empty store, because an empty
//! result is a result (§4a).

use serde::Serialize;

use crate::model::{Activity, Project, Question, Section};

#[derive(Serialize)]
struct Questions<'a> {
    degraded: Option<&'a str>,
    questions: &'a [Question],
}

#[derive(Serialize)]
struct Activities<'a> {
    degraded: Option<&'a str>,
    items: &'a [Activity],
}

#[derive(Serialize)]
struct Projects<'a> {
    degraded: Option<&'a str>,
    projects: &'a [Project],
}

pub fn questions(section: &Section<Question>) -> String {
    render(&Questions {
        degraded: section.degraded.as_deref(),
        questions: &section.rows,
    })
}

pub fn activity(section: &Section<Activity>) -> String {
    render(&Activities {
        degraded: section.degraded.as_deref(),
        items: &section.rows,
    })
}

pub fn projects(section: &Section<Project>) -> String {
    render(&Projects {
        degraded: section.degraded.as_deref(),
        projects: &section.rows,
    })
}

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

/// Serialising these cannot fail — every field is a String, an Option<String>
/// or a Vec of those — but a `/api/*` route that panicked would take a
/// connection thread with it, so the failure is a document too.
fn render<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value)
        .unwrap_or_else(|e| format!("{{\"degraded\":\"could not serialise: {e}\"}}"))
}
