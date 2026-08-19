//! Everything the workflow asks mem. Process state lives there and nowhere
//! else, and the registry is never re-implemented here (spec §3).
//!
//! A filtered read that matches nothing prints `{"items":[]}` and exits 1
//! (mem spec §7), so the array is the answer and the exit code is not.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub root: Option<String>,
    #[serde(default)]
    pub verify: Option<String>,
}

impl Project {
    /// The project name as a single path component: it names directories under
    /// the run and worktree roots.
    pub fn dir_name(&self) -> String {
        self.name.replace('/', "-")
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Item {
    pub path: String,
}

#[derive(Debug, Deserialize)]
struct Items {
    items: Vec<Item>,
}

pub fn bin() -> String {
    match std::env::var("WORKFLOW_MEM") {
        Ok(v) if !v.is_empty() => v,
        _ => "mem".to_string(),
    }
}

fn capture(args: &[&str]) -> Option<(bool, String)> {
    let out = Command::new(bin())
        .args(args)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    Some((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
    ))
}

fn silent(args: &[&str]) -> bool {
    Command::new(bin())
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Who owns this checkout. Exit 1 there means unknown, which is a fine answer:
/// the caller decides what to do without an identity.
pub fn project_current() -> Option<Project> {
    let (ok, out) = capture(&["project", "current", "--json"])?;
    if !ok {
        return None;
    }
    let p: Project = serde_json::from_str(&out).ok()?;
    if p.id.is_empty() { None } else { Some(p) }
}

/// Does this directory belong to a project mem knows? The hook's half of the
/// fire condition, and no JSON is needed to answer it.
pub fn knows_this_checkout() -> bool {
    silent(&["project", "current"])
}

fn rulings(rtype: &str, since: Option<&str>) -> Vec<Item> {
    let mut args: Vec<String> = vec![
        "log".into(),
        "--kind".into(),
        "ruling".into(),
        "--type".into(),
        rtype.into(),
    ];
    if let Some(s) = since {
        args.push("--since".into());
        args.push(s.into());
    }
    args.push("--limit".into());
    args.push("100".into());
    args.push("--json".into());
    let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let Some((_, out)) = capture(&refs) else {
        return Vec::new();
    };
    serde_json::from_str::<Items>(&out)
        .map(|i| i.items)
        .unwrap_or_default()
}

pub fn has_ruling(rtype: &str, since: Option<&str>) -> bool {
    !rulings(rtype, since).is_empty()
}

/// The bodies of every ruling of a type, run together. What a ruling *says* is
/// what clears a named term or a named test.
pub fn ruling_bodies(rtype: &str) -> String {
    let mut body = String::new();
    for item in rulings(rtype, None) {
        if let Ok(text) = std::fs::read_to_string(PathBuf::from(&item.path)) {
            body.push_str(&text);
            body.push('\n');
        }
    }
    body
}

pub fn log(text: &str) {
    silent(&["log", text]);
}

pub fn ask(text: &str) {
    silent(&["ask", text]);
}

pub fn plan_tick(task: &str) -> bool {
    silent(&["plan", "--tick", task])
}

/// This project's plan, as mem holds it.
pub fn plan() -> Option<String> {
    let (_, out) = capture(&["plan"])?;
    if out.trim().is_empty() {
        None
    } else {
        Some(out)
    }
}
