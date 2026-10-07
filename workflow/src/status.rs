//! `workflow status` -- the run dir read out loud, machine-readably on
//! request. The session that owns a run polls this instead of tailing the
//! run's stderr; policy stays in `run`, and nothing here writes.

use std::path::{Path, PathBuf};

use crate::gitcmd::Git;
use crate::{dogfood, exit, memcli, paths, plan, request, run, serve, warn};

struct TaskRow {
    id: String,
    state: String,
    dispatches: u64,
    session: String,
    failed: String,
    /// Minutes since the task's last dispatch, `-` before its first.
    age: String,
    /// The model the task runs on: its own `.model`, else the run's.
    model: String,
    /// Whole minutes the task has been out, 0 unless it is dispatched now.
    minutes: i64,
    last_status: String,
    merged: String,
    /// What the worker was carrying at its last turn, when the run could see
    /// it. Plan-sizing feedback, not a ceiling.
    context: u64,
    /// Why a task that could go is not going yet: its dependencies, a
    /// worker slot. Empty when nothing holds it.
    held: String,
}

struct RunRow {
    plan: String,
    live: bool,
    base: String,
    integration: String,
    tasks: Vec<TaskRow>,
    /// Context carried across every task, summed.
    context: u64,
}

/// What serve and mem say about the project beside its runs.
struct Serving {
    /// The serve stage file, else `execution` with a run live, else `idle`.
    stage: String,
    /// The open milestone and its place, `n of m`.
    milestone: Option<(String, usize, usize)>,
    /// `(task, reason)` for every `<task>.parked` in a run dir.
    parked: Vec<(String, String)>,
    findings: usize,
    runner: Option<String>,
    paused: bool,
    /// The leads, walk and research serve has going for the project.
    sessions: Vec<Session>,
}

/// One session serve started and has not yet ended, named the way its cost
/// line will name it.
struct Session {
    kind: String,
    name: String,
    /// Whole minutes since it started.
    minutes: i64,
}

fn field(dir: &Path, task: &str, ext: &str) -> String {
    std::fs::read_to_string(dir.join(format!("{task}.{ext}")))
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// The worker's own last report, state and note as one line, the state
/// bare of the colon a worker punctuates it with (`run::split_state`).
fn last_status(dir: &Path, task: &str) -> String {
    let text = std::fs::read_to_string(dir.join(format!("{task}.status"))).unwrap_or_default();
    match run::last_status_in(&text) {
        Some((state, note)) if note.is_empty() => state,
        Some((state, note)) => format!("{state} {note}"),
        None => String::new(),
    }
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
    let tasks: Vec<TaskRow> = task_ids(dir)
        .into_iter()
        .map(|id| TaskRow {
            state: field(dir, &id, "state"),
            dispatches: field(dir, &id, "dispatches").parse().unwrap_or(0),
            session: field(dir, &id, "session"),
            failed: field(dir, &id, "failed"),
            age: match field(dir, &id, "dispatched_at").parse::<i64>() {
                Ok(t) if t > 0 => format!("{}m", (crate::sys::now() - t).max(0) / 60),
                _ => String::from("-"),
            },
            model: match field(dir, &id, "model") {
                m if m.is_empty() => std::fs::read_to_string(dir.join("model"))
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
                m => m,
            },
            minutes: match field(dir, &id, "dispatched_at").parse::<i64>() {
                Ok(t) if t > 0 && field(dir, &id, "state") == run::DISPATCHED => {
                    (crate::sys::now() - t).max(0) / 60
                }
                _ => 0,
            },
            last_status: last_status(dir, &id),
            merged: field(dir, &id, "merged"),
            context: field(dir, &id, "context").parse().unwrap_or(0),
            held: field(dir, &id, "held"),
            id,
        })
        .collect();
    let context: u64 = tasks.iter().map(|t| t.context).sum();
    Some(RunRow {
        live: live(dir),
        base: std::fs::read_to_string(dir.join("base_sha"))
            .unwrap_or_default()
            .trim()
            .to_string(),
        integration: format!("integration/{plan_id}"),
        tasks,
        context,
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

/// Every task a lead parked in one of the project's run dirs, with the
/// reason it gave.
fn parked(project_dir: &str) -> Vec<(String, String)> {
    let root = paths::runs_root().join(project_dir);
    let mut found: Vec<(String, String)> = std::fs::read_dir(&root)
        .into_iter()
        .flatten()
        .flatten()
        .flat_map(|run| {
            std::fs::read_dir(run.path())
                .into_iter()
                .flatten()
                .flatten()
        })
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let task = name.strip_suffix(".parked")?.to_string();
            let reason = std::fs::read_to_string(e.path()).unwrap_or_default();
            Some((task, reason.trim().to_string()))
        })
        .collect();
    found.sort();
    found
}

/// The sessions recorded in a project's serve dir: each line of `leads`,
/// the `walk` until serve has read what it came to, and the `research`.
/// A project serve never touched has no dir and so lists none.
fn sessions(dir: &Path, project: &str) -> Vec<Session> {
    let read = |name: &str| std::fs::read_to_string(dir.join(name)).unwrap_or_default();
    let minutes = |started: i64| (crate::sys::now() - started).max(0) / 60;
    // serve keeps its own parser private; the line is
    // `<kind> <session> <started> <slug>`.
    let mut found: Vec<Session> = read("leads")
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let kind = parts.next()?.to_string();
            let started: i64 = parts.nth(1)?.parse().ok()?;
            Some(Session {
                kind,
                name: parts.next().unwrap_or_default().to_string(),
                minutes: minutes(started),
            })
        })
        .collect();
    if let Some(walk) = dogfood::WalkState::read(&read("walk"))
        && walk.outcome.is_none()
    {
        found.push(Session {
            kind: "walk".into(),
            name: walk.slug,
            minutes: minutes(walk.started),
        });
    }
    if let Some(research) = request::AskedResearch::read(&read("research")) {
        found.push(Session {
            kind: "research".into(),
            name: project.to_string(),
            minutes: minutes(research.started),
        });
    }
    found
}

fn serving(project: &memcli::Project, rows: &[RunRow]) -> Serving {
    let stage = serve::stage_of(&project.name).unwrap_or_else(|| {
        if rows.iter().any(|r| r.live) {
            "execution".to_string()
        } else {
            "idle".to_string()
        }
    });
    Serving {
        stage,
        milestone: serve::roadmap_place(&project.name),
        parked: parked(&project.dir_name()),
        findings: serve::open_findings(&project.name),
        runner: memcli::project_choice("runner"),
        paused: memcli::project_choice("paused").is_some(),
        sessions: sessions(&paths::serve_root().join(project.dir_name()), &project.name),
    }
}

fn as_json(project: &str, serving: &Serving, rows: &[RunRow]) -> serde_json::Value {
    serde_json::json!({
        "project": project,
        "stage": serving.stage,
        "milestone": serving.milestone.as_ref().map(|(slug, n, m)| serde_json::json!({
            "slug": slug,
            "n": n,
            "m": m,
        })),
        "parked": serving.parked.iter().map(|(task, reason)| serde_json::json!({
            "task": task,
            "reason": reason,
        })).collect::<Vec<_>>(),
        "findings": serving.findings,
        "runner": serving.runner,
        "paused": serving.paused,
        "sessions": serving.sessions.iter().map(|s| serde_json::json!({
            "kind": s.kind,
            "name": s.name,
            "minutes": s.minutes,
        })).collect::<Vec<_>>(),
        "runs": rows.iter().map(|r| serde_json::json!({
            "plan": r.plan,
            "live": r.live,
            "base": r.base,
            "integration": r.integration,
            "context": r.context,
            "tasks": r.tasks.iter().map(|t| serde_json::json!({
                "id": t.id,
                "state": t.state,
                "dispatches": t.dispatches,
                "session": t.session,
                "failed": t.failed,
                "last_status": t.last_status,
                "merged": t.merged,
                "context": t.context,
                "held": t.held,
                "model": t.model,
                "minutes": t.minutes,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

/// The line the human report opens with; a milestone or a runner that is
/// not there is left out rather than named as nothing.
fn serving_line(s: &Serving) -> String {
    let mut parts = vec![format!("stage: {}", s.stage)];
    if let Some((slug, n, m)) = &s.milestone {
        parts.push(format!("milestone {slug} ({n} of {m})"));
    }
    if let Some(runner) = &s.runner {
        parts.push(format!("runner {runner}"));
    }
    parts.push(format!("{} parked", s.parked.len()));
    parts.join(" · ")
}

fn print_human(rows: &[RunRow]) {
    for r in rows {
        println!(
            "run {} ({}, {})",
            r.plan,
            if r.live {
                "live"
            } else {
                "nobody at the wheel"
            },
            r.integration
        );
        for t in &r.tasks {
            let mut detail = String::new();
            // A reason only while it is the task's state: a merged task
            // whose .failed still held its first attempt's refusal showed
            // that refusal on every status read after, fifty-six times in
            // one session.
            if !t.held.is_empty() {
                detail = t.held.clone();
            } else if !t.failed.is_empty() && (t.state == run::FAILED || t.state == run::BLOCKED) {
                detail = t.failed.clone();
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

/// One line per task, state and age since its last dispatch, nothing else:
/// what an orchestrator polls with, since the full report re-entered its
/// window sixty times in one session.
fn print_brief(rows: &[RunRow]) {
    for r in rows {
        println!("run {} ({})", r.plan, if r.live { "live" } else { "ended" });
        for t in &r.tasks {
            println!("  {:<8} {:<10} {}", t.id, t.state, t.age);
        }
    }
}

pub fn cmd_status(json: bool, brief: bool) -> i32 {
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
        let serving = serving(&project, &rows);
        println!(
            "{}",
            serde_json::to_string_pretty(&as_json(&project.name, &serving, &rows))
                .unwrap_or_else(|_| "{}".into())
        );
        return exit::OK;
    }
    if !brief {
        println!("{}", serving_line(&serving(&project, &rows)));
    }
    if rows.is_empty() {
        warn(format!("status: no runs recorded for {}", project.name));
        return exit::OK;
    }
    if brief {
        print_brief(&rows);
    } else {
        print_human(&rows);
    }
    exit::OK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_json_carries_no_reader_counts() {
        let rows = [RunRow {
            plan: "demo".into(),
            live: false,
            base: "abc123".into(),
            integration: "integration/demo".into(),
            tasks: vec![TaskRow {
                id: "t1".into(),
                state: "merged".into(),
                dispatches: 1,
                session: String::new(),
                failed: String::new(),
                age: "-".into(),
                model: String::new(),
                minutes: 0,
                last_status: String::new(),
                merged: "deadbeef".into(),
                context: 4000,
                held: String::new(),
            }],
            context: 4000,
        }];
        let serving = Serving {
            stage: "idle".into(),
            milestone: None,
            parked: Vec::new(),
            findings: 0,
            runner: None,
            paused: false,
            sessions: Vec::new(),
        };
        let doc = as_json("app", &serving, &rows);
        let run = &doc["runs"][0];
        assert_eq!(run["context"], 4000);
        assert!(run.get("readings").is_none(), "{run}");
        assert!(run.get("fixes").is_none(), "{run}");
        assert_eq!(run["tasks"][0]["state"], "merged");
        assert!(run["tasks"][0].get("reviews").is_none(), "{run}");
    }

    #[test]
    fn status_json_carries_the_serve_fields() {
        let serving = Serving {
            stage: "waiting".into(),
            milestone: Some(("m2".into(), 2, 3)),
            parked: vec![("ask".into(), "the owner decides".into())],
            findings: 4,
            runner: Some("here".into()),
            paused: true,
            sessions: Vec::new(),
        };
        let doc = as_json("app", &serving, &[]);
        assert_eq!(doc["stage"], "waiting");
        assert_eq!(
            doc["milestone"],
            serde_json::json!({"slug": "m2", "n": 2, "m": 3})
        );
        assert_eq!(
            doc["parked"],
            serde_json::json!([{"task": "ask", "reason": "the owner decides"}])
        );
        assert_eq!(doc["findings"], 4);
        assert_eq!(doc["runner"], "here");
        assert_eq!(doc["paused"], true);
        assert_eq!(
            serving_line(&serving),
            "stage: waiting · milestone m2 (2 of 3) · runner here · 1 parked"
        );
        let idle = Serving {
            stage: "idle".into(),
            milestone: None,
            parked: Vec::new(),
            findings: 0,
            runner: None,
            paused: false,
            sessions: Vec::new(),
        };
        let doc = as_json("app", &idle, &[]);
        assert!(doc["milestone"].is_null() && doc["runner"].is_null());
        assert_eq!(serving_line(&idle), "stage: idle · 0 parked");
    }

    #[test]
    fn status_json_names_each_tasks_model_and_minutes() {
        let dir = std::env::temp_dir().join(format!("wf-status-agents-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("plan.md"),
            "# plan: demo\n\n- [ ] t1 One\n      Files: a\n      Verify: true\n\
             - [ ] t2 Two\n      Files: b\n      Verify: true\n\
             - [ ] t3 Three\n      Files: c\n      Verify: true\n",
        )
        .unwrap();
        std::fs::write(dir.join("model"), "run-model\n").unwrap();
        let three_ago = (crate::sys::now() - 180).to_string();
        for (task, state) in [("t1", "dispatched"), ("t2", "dispatched"), ("t3", "merged")] {
            std::fs::write(dir.join(format!("{task}.state")), state).unwrap();
            std::fs::write(dir.join(format!("{task}.dispatched_at")), &three_ago).unwrap();
        }
        std::fs::write(dir.join("t1.model"), "task-model\n").unwrap();
        let row = read_run(&dir).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let doc = as_json(
            "app",
            &Serving {
                stage: "idle".into(),
                milestone: None,
                parked: Vec::new(),
                findings: 0,
                runner: None,
                paused: false,
                sessions: Vec::new(),
            },
            &[row],
        );
        let tasks = &doc["runs"][0]["tasks"];
        assert_eq!(tasks[0]["model"], "task-model");
        assert_eq!(tasks[0]["minutes"], 3);
        assert_eq!(tasks[1]["model"], "run-model");
        assert_eq!(tasks[1]["minutes"], 3);
        assert_eq!(tasks[2]["minutes"], 0);
    }

    #[test]
    fn status_json_lists_the_sessions_serve_has_going() {
        let dir = std::env::temp_dir().join(format!("wf-status-sessions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let three_ago = crate::sys::now() - 180;
        std::fs::write(
            dir.join("leads"),
            format!("pickup s1 {three_ago} m1\nquestion s2 {three_ago} m1\n"),
        )
        .unwrap();
        std::fs::write(dir.join("walk"), format!("m1 1 {three_ago} s3 1 2\n")).unwrap();
        let sessions = sessions(&dir, "app");
        let _ = std::fs::remove_dir_all(&dir);
        let doc = as_json(
            "app",
            &Serving {
                stage: "dogfood".into(),
                milestone: None,
                parked: Vec::new(),
                findings: 0,
                runner: None,
                paused: false,
                sessions,
            },
            &[],
        );
        assert_eq!(
            doc["sessions"],
            serde_json::json!([
                {"kind": "pickup", "name": "m1", "minutes": 3},
                {"kind": "question", "name": "m1", "minutes": 3},
                {"kind": "walk", "name": "m1", "minutes": 3},
            ])
        );
        let none = std::env::temp_dir().join(format!("wf-status-no-serve-{}", std::process::id()));
        assert!(super::sessions(&none, "app").is_empty());
    }
}
