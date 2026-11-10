//! Everything the workflow asks mem. Process state lives there and nowhere
//! else, and the registry is never re-implemented here.
//!
//! A filtered read that matches nothing prints `{"items":[]}` and exits 1, so
//! the array is the answer and the exit code is not.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde::Deserialize;

/// The project a directory belongs to, as `mem project current --json` names
/// it.
#[derive(Debug, Clone, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    /// The checkout the answer was read in; null when mem was told the project
    /// by name from outside one.
    pub root: Option<String>,
    /// Where a child project lives under `root`; absent on a root project.
    #[serde(default)]
    pub subdir: Option<String>,
}

/// One project of `mem projects --json`: only what `go` reads of it.
#[derive(Debug, Clone, Deserialize)]
pub struct ProjectRow {
    pub name: String,
    /// The paths of this project's checkouts on this machine.
    #[serde(default)]
    pub checkouts: Vec<String>,
    #[serde(default)]
    pub roadmap_status: Option<String>,
    /// Where a child project lives under its root's checkout.
    #[serde(default)]
    pub subdir: Option<String>,
    /// The linked worktree recorded for it on this machine.
    #[serde(default)]
    pub worktree: Option<String>,
    /// "<when> by <who>" while the project is paused.
    #[serde(default)]
    pub paused: Option<String>,
    /// The machine whose local hub carries this project's milestones to the
    /// cloud (`mem project set cloud`).
    #[serde(default)]
    pub cloud: Option<String>,
    /// The machine and amx session running it, while one does.
    #[serde(default)]
    pub runner: Option<Runner>,
    /// Its origin remote, normalized; none for a project without one.
    #[serde(default)]
    pub remote: Option<String>,
    /// The roadmap's first open milestone.
    #[serde(default)]
    pub milestone: Option<String>,
    /// When the latest handoff was written, RFC 3339.
    #[serde(default)]
    pub handoff_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Runner {
    pub machine: String,
    pub id: String,
    #[serde(default)]
    pub since: Option<String>,
    /// A cloud run's milestone.
    #[serde(default)]
    pub milestone: Option<String>,
    /// The claude.ai account a cloud run's Project runs on, "work" or "main".
    #[serde(default)]
    pub account: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Projects {
    projects: Vec<ProjectRow>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Item {
    pub path: String,
}

#[derive(Debug, Deserialize)]
struct Items {
    items: Vec<Item>,
}

/// A question as `mem show <id> --json` and `mem questions --json` print it:
/// only what a thread's wait and its start read of it.
#[derive(Debug, Clone, Deserialize)]
pub struct Question {
    pub id: String,
    pub short_id: String,
    #[serde(default)]
    pub answered: bool,
    #[serde(default)]
    pub answer: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
}

impl Question {
    /// When it was asked, read from its ULID: mem's `created` is a date.
    pub fn asked(&self) -> Option<jiff::Timestamp> {
        const CROCKFORD: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
        let mut ms: i64 = 0;
        for c in self.id.get(..10)?.bytes() {
            let digit = CROCKFORD
                .iter()
                .position(|&d| d == c.to_ascii_uppercase())?;
            ms = ms * 32 + digit as i64;
        }
        jiff::Timestamp::from_millisecond(ms).ok()
    }
}

#[derive(Debug, Deserialize)]
struct Questions {
    items: Vec<Question>,
}

/// The question `id` names, `None` when mem knows none.
pub fn question(id: &str) -> Option<Question> {
    let (ok, out) = capture(&["show", id, "--json"])?;
    if !ok {
        return None;
    }
    let mut items = serde_json::from_str::<Questions>(&out).ok()?.items;
    (items.len() == 1).then(|| items.remove(0))
}

/// The file at the top of a checkout that carries the writes mem's store has
/// not taken yet, `mem share export`'s.
pub const OUTBOX_FILE: &str = ".mem-outbox.json";

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

/// The project mem files the working directory under. Exit 1 there means
/// unknown, which is a fine answer: the caller decides what to do without an
/// identity.
pub fn project_current() -> Option<Project> {
    let (ok, out) = capture(&["project", "current", "--json"])?;
    if !ok {
        return None;
    }
    let p: Project = serde_json::from_str(&out).ok()?;
    if p.id.is_empty() { None } else { Some(p) }
}

/// Every project of the store, `None` when mem cannot say.
pub fn projects() -> Option<Vec<ProjectRow>> {
    let (ok, out) = capture(&["projects", "--json"])?;
    if !ok {
        return None;
    }
    parse_projects(&out)
}

pub fn parse_projects(text: &str) -> Option<Vec<ProjectRow>> {
    serde_json::from_str::<Projects>(text)
        .ok()
        .map(|p| p.projects)
}

/// A project's roadmap text, verbatim. `None` when mem cannot be run or
/// refuses; a project with no roadmap prints nothing, which is an empty text.
pub fn roadmap(project: &str) -> Option<String> {
    let (ok, out) = capture(&["roadmap", "--project", project])?;
    ok.then_some(out)
}

/// A project's latest handoff, verbatim. `None` when there is none or mem
/// cannot say.
pub fn handoff(project: &str) -> Option<String> {
    let (ok, out) = capture(&["handoff", "--project", project])?;
    ok.then_some(out)
}

/// `mem project set paused <who>`: mem stamps the time.
pub fn set_paused(project: &str, who: &str) -> bool {
    silent(&["project", "set", "paused", who, "--project", project])
}

/// `mem project set runner <machine> <id>`: mem stamps the time.
pub fn set_runner(project: &str, machine: &str, id: &str) -> bool {
    silent(&[
        "project",
        "set",
        "runner",
        machine,
        id,
        "--project",
        project,
    ])
}

/// `mem project set runner cloud <session> --milestone <slug>`, with the
/// claude.ai account the environment's `WORKFLOW_ACCOUNT` names, when it
/// names one.
pub fn set_cloud_runner(project: &str, session: &str, slug: &str) -> bool {
    let account = std::env::var("WORKFLOW_ACCOUNT").unwrap_or_default();
    let account = account.trim();
    let mut args = vec![
        "project",
        "set",
        "runner",
        "cloud",
        session,
        "--milestone",
        slug,
        "--project",
        project,
    ];
    if !account.is_empty() {
        args.extend(["--account", account]);
    }
    silent(&args)
}

/// `mem sync`: one round with the hub's store, done when it says performed.
pub fn sync() -> bool {
    capture(&["sync", "--json"]).is_some_and(|(ok, out)| {
        ok && serde_json::from_str::<serde_json::Value>(&out)
            .is_ok_and(|doc| doc["sync"] == "performed")
    })
}

/// `mem sync`, and how many writes still wait in the outbox after it: 0
/// once it says performed, which it says only with none waiting. `None`
/// when mem fails, or the store's writes stopped landing here, so that what
/// this replica reads cannot be trusted either.
pub fn sync_outbox() -> Option<u64> {
    let (ok, out) = capture(&["sync", "--json"])?;
    let doc: serde_json::Value = serde_json::from_str(&out).ok()?;
    if !ok {
        return None;
    }
    if doc["sync"] == "performed" {
        return Some(0);
    }
    doc["outbox"].as_u64().filter(|n| *n > 0)
}

/// `mem roadmap --tick <slug>`.
pub fn tick(project: &str, slug: &str) -> bool {
    silent(&["roadmap", "--tick", slug, "--project", project])
}

/// `mem share export`: the writes the store has not taken, staged in the
/// checkout's outbox file.
pub fn share_export() -> bool {
    silent(&["share", "export"])
}

pub fn unset_runner(project: &str) -> bool {
    silent(&["project", "unset", "runner", "--project", project])
}

pub fn unset_paused(project: &str) -> bool {
    silent(&["project", "unset", "paused", "--project", project])
}

/// A choice this project declared with `mem project set <key>`.
///
/// Read straight out of the document rather than modelled on [`Project`],
/// which is how mem holds it: mem stores the choice and hands it to whoever
/// needs it. `None` covers both an unregistered checkout and one that never
/// chose.
pub fn project_choice(key: &str) -> Option<String> {
    let (ok, out) = capture(&["project", "current", "--json"])?;
    if !ok {
        return None;
    }
    let doc: serde_json::Value = serde_json::from_str(&out).ok()?;
    let name = doc.get(key)?.as_str()?.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// The path globs this project keeps out of the hygiene check, `mem project
/// set hygiene-exempt`, split the way a Files line is.
pub fn project_hygiene_exempt() -> Vec<String> {
    project_choice("hygiene_exempt")
        .map(|globs| split_patterns(&globs))
        .unwrap_or_default()
}

/// Whitespace-separated globs, double quotes around one that contains a space.
pub fn split_patterns(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    for c in line.chars() {
        match c {
            '"' => in_quotes = !in_quotes,
            c if c.is_ascii_whitespace() && !in_quotes => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Does this directory belong to a project mem knows? The hook's half of the
/// fire condition, and no JSON is needed to answer it.
pub fn knows_this_checkout() -> bool {
    silent(&["project", "current"])
}

fn rulings(rtype: &str) -> Vec<Item> {
    let Some((_, out)) = capture(&[
        "log", "--kind", "ruling", "--type", rtype, "--limit", "100", "--json",
    ]) else {
        return Vec::new();
    };
    serde_json::from_str::<Items>(&out)
        .map(|i| i.items)
        .unwrap_or_default()
}

/// The bodies of every ruling of a type, run together. What a ruling *says* is
/// what clears a named term.
pub fn ruling_bodies(rtype: &str) -> String {
    let mut body = String::new();
    for item in rulings(rtype) {
        if let Ok(text) = std::fs::read_to_string(PathBuf::from(&item.path)) {
            body.push_str(&text);
            body.push('\n');
        }
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trailing_newline_does_not_eat_the_last_pattern() {
        let line = "apps/admin/src/** pnpm-lock.yaml apps/platform/test/storefront.test.ts\n";
        assert_eq!(
            split_patterns(line),
            vec![
                "apps/admin/src/**",
                "pnpm-lock.yaml",
                "apps/platform/test/storefront.test.ts"
            ]
        );
        assert_eq!(split_patterns("a.rs\n\nb.rs\r\n"), vec!["a.rs", "b.rs"]);
    }

    #[test]
    fn quotes_keep_a_pattern_with_a_space_whole() {
        assert_eq!(
            split_patterns("a/b \"one path/with space.php\" c"),
            vec!["a/b", "one path/with space.php", "c"]
        );
        assert_eq!(
            split_patterns("\"one file.rs\" two.rs\n"),
            vec!["one file.rs", "two.rs"]
        );
        assert!(split_patterns("   ").is_empty());
    }

    #[test]
    fn the_projects_listing_reads_only_what_go_needs() {
        let text = r#"{"projects":[
            {"name":"alpha","checkouts":["/a","/b"],"roadmap_status":"approved","items":3},
            {"name":"beta","checkouts":[],"roadmap_status":null}
        ]}"#;
        let rows = parse_projects(text).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "alpha");
        assert_eq!(rows[0].checkouts, ["/a", "/b"]);
        assert_eq!(rows[0].roadmap_status.as_deref(), Some("approved"));
        assert!(rows[1].checkouts.is_empty());
        assert_eq!(rows[1].roadmap_status, None);
        assert!(parse_projects("not json").is_none());
    }
}
