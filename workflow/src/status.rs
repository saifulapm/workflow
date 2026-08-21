//! `workflow status` -- the run dir read out loud, machine-readably on
//! request. The session that owns a run polls this instead of tailing the
//! run's stderr; policy stays in `run`, and nothing here writes.

use std::path::{Path, PathBuf};

use crate::gitcmd::Git;
use crate::{exit, memcli, paths, plan, run, warn};

struct TaskRow {
    id: String,
    state: String,
    dispatches: u64,
    session: String,
    parked: String,
    last_status: String,
    merged: String,
    /// What the worker was carrying at its last turn, when the run could see
    /// it. Plan-sizing feedback, not a ceiling (ruling #D7A4T2CH).
    context: u64,
}

struct RunRow {
    plan: String,
    live: bool,
    base: String,
    integration: String,
    tasks: Vec<TaskRow>,
}

fn field(dir: &Path, task: &str, ext: &str) -> String {
    std::fs::read_to_string(dir.join(format!("{task}.{ext}")))
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// The worker's own last report, state and note as one line.
fn last_status(dir: &Path, task: &str) -> String {
    let text = std::fs::read_to_string(dir.join(format!("{task}.status"))).unwrap_or_default();
    let mut last = String::new();
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let (Some(_utc), Some(state)) = (fields.next(), fields.next()) else {
            continue;
        };
        let note = fields.collect::<Vec<_>>().join(" ");
        last = if note.is_empty() {
            state.to_string()
        } else {
            format!("{state} {note}")
        };
    }
    last
}

/// The task ids in plan order when the recorded plan still parses, and from
/// the state files on disk when it does not: the dir is the ground truth.
fn task_ids(dir: &Path) -> Vec<String> {
    if let Ok(text) = std::fs::read_to_string(dir.join("plan.md"))
        && let Some(parsed) = plan::parse(&text, false)
    {
        return parsed.ids();
    }
    let mut ids: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.strip_suffix(".state").map(str::to_string)
        })
        .collect();
    ids.sort();
    ids
}

/// Is an orchestrator holding this run right now? Probed with the run lock
/// itself, taken and released in one motion. The window in which this probe
/// holds the lock is microseconds; a run starting in exactly that window
/// would refuse and say another orchestrator is live, which is the probe's
/// one known cost.
fn live(dir: &Path) -> bool {
    run::lock_run(dir).is_none()
}

fn read_run(dir: &Path) -> Option<RunRow> {
    let plan_id = dir.file_name()?.to_string_lossy().to_string();
    if !dir.join("plan.md").is_file() {
        return None;
    }
    let tasks = task_ids(dir)
        .into_iter()
        .map(|id| TaskRow {
            state: field(dir, &id, "state"),
            dispatches: field(dir, &id, "dispatches").parse().unwrap_or(0),
            session: field(dir, &id, "session"),
            parked: field(dir, &id, "parked"),
            last_status: last_status(dir, &id),
            merged: field(dir, &id, "merged"),
            context: field(dir, &id, "context").parse().unwrap_or(0),
            id,
        })
        .collect();
    Some(RunRow {
        live: live(dir),
        base: std::fs::read_to_string(dir.join("base_sha"))
            .unwrap_or_default()
            .trim()
            .to_string(),
        integration: format!("integration/{plan_id}"),
        tasks,
        plan: plan_id,
    })
}

fn runs(project_dir: &str) -> Vec<RunRow> {
    let root = paths::runs_root().join(project_dir);
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs.iter().filter_map(|d| read_run(d)).collect()
}

fn as_json(project: &str, rows: &[RunRow]) -> serde_json::Value {
    serde_json::json!({
        "project": project,
        "runs": rows.iter().map(|r| serde_json::json!({
            "plan": r.plan,
            "live": r.live,
            "base": r.base,
            "integration": r.integration,
            "tasks": r.tasks.iter().map(|t| serde_json::json!({
                "id": t.id,
                "state": t.state,
                "dispatches": t.dispatches,
                "session": t.session,
                "parked": t.parked,
                "last_status": t.last_status,
                "merged": t.merged,
                "context": t.context,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

fn print_human(rows: &[RunRow]) {
    for r in rows {
        println!(
            "run {} ({}, {})",
            r.plan,
            if r.live { "live" } else { "nobody at the wheel" },
            r.integration
        );
        for t in &r.tasks {
            let mut detail = String::new();
            if !t.parked.is_empty() {
                detail = t.parked.clone();
            } else if !t.last_status.is_empty() {
                detail = format!("last report: {}", t.last_status);
            } else if !t.merged.is_empty() {
                detail = format!("merged {}", &t.merged[..t.merged.len().min(12)]);
            }
            if t.context > 0 {
                let carried = format!("carried {}", run::tokens(t.context));
                detail = if detail.is_empty() {
                    carried
                } else {
                    format!("{detail} ({carried})")
                };
            }
            println!("  {:<8} {:<10} {}", t.id, t.state, detail);
        }
    }
}

pub fn cmd_status(json: bool) -> i32 {
    if !Git::here().inside_worktree() {
        warn("status: stand in the project checkout");
        return exit::USAGE;
    }
    let Some(project) = memcli::project_current() else {
        warn("status: mem does not know this checkout, so there is no project to report on");
        return exit::USAGE;
    };
    let rows = runs(&project.dir_name());
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&as_json(&project.name, &rows))
                .unwrap_or_else(|_| "{}".into())
        );
        return exit::OK;
    }
    if rows.is_empty() {
        warn(format!("status: no runs recorded for {}", project.name));
        return exit::OK;
    }
    print_human(&rows);
    exit::OK
}
