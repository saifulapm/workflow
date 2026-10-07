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
