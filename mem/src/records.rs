//! The record verbs: decisions, evidence, findings, raw sources, the brief and
//! ideas. Each record is one item file written once, so sync never sees two
//! machines edit the same file; closing a finding is the one in-place edit,
//! made the way prune archives. Files a record points at are copied into the
//! project's directory so they sync with it, and `file` holds the path
//! relative to that directory, which is the same on every machine.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use jiff::Timestamp;
use serde_json::json;

use crate::app::App;
use crate::exit;
use crate::ids::IdRef;
use crate::index::Row;
use crate::item::{Item, Kind, Meta};
use crate::project::{Identity, Mode};
use crate::write::{Written, derive_title, write_item};

/// No list verb here takes a limit: a project's records are few enough to
/// print whole.
const ALL: usize = i64::MAX as usize;

/// `mem decide "<text>" --by <who> [--replaces "<text>"]`.
pub fn decide(app: &App, text: &str, by: &str, replaces: Option<&str>) -> Result<i32> {
    let identity = app.identity(Mode::Write)?;
    let mut meta = new_meta(app, Kind::Ruling, text);
    meta.by = Some(by.to_string());
    meta.replaces = replaces.map(|r| r.to_string());
    let written = write_item(app, &identity, meta, text.to_string())?;
    report(app, &written, Kind::Ruling)
}

/// `mem evidence add --task <id> <file> --note "<text>"`.
pub fn evidence_add(app: &App, task: &str, file: &Path, note: &str) -> Result<i32> {
    let (identity, dir) = project(app, "evidence")?;
    let rel = format!("evidence/{}/{}", dir_name(task, "task")?, base_name(file)?);
    copy_in(app, &dir, &rel, file)?;
    let mut meta = new_meta(app, Kind::Evidence, note);
    meta.task = Some(task.to_string());
    meta.file = Some(rel);
    let written = write_item(app, &identity, meta, note.to_string())?;
    report(app, &written, Kind::Evidence)
}

/// `mem evidence list [--task <id>]`, newest first.
pub fn evidence_list(app: &App, task: Option<&str>) -> Result<i32> {
    let mut rows = rows_of(app, Kind::Evidence)?;
    if let Some(task) = task {
        rows.retain(|(row, _)| row.task.as_deref() == Some(task));
    }
    let print = |row: &Row, item: &Item| {
        format!(
            "#{}  {}  {}  {}",
            row.short_id,
            item.meta.task.as_deref().unwrap_or(""),
            item.meta.file.as_deref().unwrap_or(""),
            first_line(item)
        )
    };
    let to_json = |row: &Row, item: &Item| {
        let mut v = crate::verbs::row_json(row);
        v["file"] = json!(item.meta.file);
        v["note"] = json!(item.body_str().trim());
        v
    };
    list(app, &rows, print, to_json)
}

/// `mem finding add --milestone <slug> --step <n> "<text>" [--evidence <file>]`.
/// The evidence file is filed by milestone, where a task's evidence is filed
/// by task: a finding belongs to a milestone's step, not to a task.
pub fn finding_add(
    app: &App,
    milestone: &str,
    step: &str,
    text: &str,
    evidence: Option<&Path>,
) -> Result<i32> {
    let (identity, dir) = project(app, "a finding")?;
    let mut meta = new_meta(app, Kind::Finding, text);
    if let Some(file) = evidence {
        let rel = format!(
            "evidence/{}/{}",
            dir_name(milestone, "milestone")?,
            base_name(file)?
        );
        copy_in(app, &dir, &rel, file)?;
        meta.file = Some(rel);
    }
    meta.milestone = Some(milestone.to_string());
    meta.step = Some(step.to_string());
    meta.status = Some("open".to_string());
    let written = write_item(app, &identity, meta, text.to_string())?;
    report(app, &written, Kind::Finding)
}

/// `mem finding list [--open]`, newest first.
pub fn finding_list(app: &App, open: bool) -> Result<i32> {
    let mut rows = rows_of(app, Kind::Finding)?;
    if open {
        rows.retain(|(_, item)| item.meta.status.as_deref() == Some("open"));
    }
    let print = |row: &Row, item: &Item| {
        format!(
            "#{}  {}  {}  {}  {}",
            row.short_id,
            item.meta.status.as_deref().unwrap_or(""),
            item.meta.milestone.as_deref().unwrap_or(""),
            item.meta.step.as_deref().unwrap_or(""),
            row.title
        )
    };
    let to_json = |row: &Row, item: &Item| {
        let mut v = crate::verbs::row_json(row);
        v["milestone"] = json!(item.meta.milestone);
        v["step"] = json!(item.meta.step);
        v["status"] = json!(item.meta.status);
        v["fixed_by"] = json!(item.meta.fixed_by);
        v["file"] = json!(item.meta.file);
        v
    };
    list(app, &rows, print, to_json)
}

/// `mem finding close <id> --by <commit>`, in place.
pub fn finding_close(app: &App, id: &str, by: &str) -> Result<i32> {
    let Some(id_ref) = IdRef::parse(id) else {
        return Err(exit::not_found(format!("'{id}' is not an id")));
    };
    let index = app.read_index()?;
    let mut rows = index.resolve_ref(&id_ref)?;
    let row = match rows.len() {
        0 => return Err(exit::not_found(format!("no finding {id}"))),
        1 => rows.remove(0),
        _ => {
            return Err(exit::coded(
                exit::AMBIGUOUS,
                format!("'{id}' is ambiguous — use the full ULID"),
            ));
        }
    };
    let mut item = crate::store::read_item(&row.path)?;
    if item.meta.kind != Kind::Finding {
        return Err(exit::usage(format!(
            "#{} is a {}, not a finding",
            row.short_id, item.meta.kind
        )));
    }
    item.meta.status = Some("fixed".to_string());
    item.meta.fixed_by = Some(by.to_string());
    item.meta.modified = Timestamp::now();
    crate::atomic::write_atomic(&row.path, &item.to_bytes()?)?;
    let written = Written {
        short_id: item.meta.short_id(),
        id: item.meta.id,
        path: row.path,
    };
    report(app, &written, Kind::Finding)
}

/// `mem raw add <file|url>`. A raw source is kept as it was taken, so a name
/// already under `raw/` is refused rather than replaced.
pub fn raw_add(app: &App, source: &str) -> Result<i32> {
    let (identity, dir) = project(app, "a raw source")?;
    let url = source.contains("://");
    let name = if url {
        let path = source.split(['?', '#']).next().unwrap_or("");
        let name = path.rsplit('/').next().unwrap_or("");
        dir_name(name, "the URL's file name")?.to_string()
    } else {
        base_name(Path::new(source))?
    };
    let rel = format!("raw/{name}");
    let dest = dir.join(&rel);
    if dest.exists() {
        return Err(exit::usage(format!(
            "{rel} is already stored, and a raw source is never replaced — rename the new one"
        )));
    }
    if url {
        fetch(app, &dest, source)?;
    } else {
        copy_in(app, &dir, &rel, Path::new(source))?;
    }
    let mut meta = new_meta(app, Kind::Raw, &name);
    meta.file = Some(rel);
    meta.source = Some(source.to_string());
    let written = write_item(app, &identity, meta, source.to_string())?;
    report(app, &written, Kind::Raw)
}

/// `mem brief --set "<text>"` writes one; `mem brief` prints the newest.
pub fn brief(app: &App, set: Option<&str>) -> Result<i32> {
    if let Some(text) = set {
        let written = crate::write::save(app, Kind::Brief, text, None, None, &[], None)?;
        return report(app, &written, Kind::Brief);
    }
    let identity = app.identity(Mode::Read)?;
    let index = app.read_index()?;
    let Some(row) = index.recent("brief", identity.id(), 1)?.into_iter().next() else {
        if !app.quiet {
            eprintln!("mem: no brief recorded for this project");
        }
        return Ok(exit::NOT_FOUND);
    };
    let body = crate::store::read_item(&row.path)?.body_str().to_string();
    if app.json {
        let mut v = crate::verbs::row_json(&row);
        v["body"] = json!(body);
        println!("{}", serde_json::to_string(&v)?);
    } else {
        println!("{}", body.trim());
    }
    Ok(exit::OK)
}

/// `mem idea "<text>"`.
pub fn idea(app: &App, text: &str) -> Result<i32> {
    let written = crate::write::save(app, Kind::Idea, text, None, None, &[], None)?;
    report(app, &written, Kind::Idea)
}

fn new_meta(app: &App, kind: Kind, text: &str) -> Meta {
    Meta::new(String::new(), kind, derive_title(text), app.machine.clone())
}

/// The write identity and its project directory. Records that carry a file
/// need somewhere to put it, and global scope has no directory of its own.
fn project(app: &App, what: &str) -> Result<(Identity, PathBuf)> {
    let identity = app.identity(Mode::Write)?;
    let Some(id) = identity.id() else {
        return Err(exit::usage(format!(
            "{what} belongs to a project — run this in a checkout or pass --project"
        )));
    };
    let dir = app.store.project_dir(id);
    Ok((identity, dir))
}

/// A value used as one directory or file name under the project: anything
/// that could climb out of it or reach deeper is refused.
fn dir_name<'a>(value: &'a str, what: &str) -> Result<&'a str> {
    if value.is_empty() || value == "." || value == ".." || value.contains(['/', '\\']) {
        return Err(exit::usage(format!(
            "{what} '{value}' cannot be used as a file name"
        )));
    }
    Ok(value)
}

fn base_name(file: &Path) -> Result<String> {
    file.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.to_string())
        .ok_or_else(|| exit::usage(format!("{} names no file", file.display())))
}

/// Copies `from` to `<dir>/<rel>`. The version check comes first so a store
/// this binary may not write gets no file without its item.
fn copy_in(app: &App, dir: &Path, rel: &str, from: &Path) -> Result<()> {
    if !from.is_file() {
        return Err(exit::not_found(format!("no file {}", from.display())));
    }
    crate::maint::check_write_version(&app.store)?;
    let dest = dir.join(rel);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::copy(from, &dest)
        .map_err(|e| exit::store_error(format!("copying {}: {e}", from.display())))?;
    Ok(())
}

fn fetch(app: &App, dest: &Path, url: &str) -> Result<()> {
    crate::maint::check_write_version(&app.store)?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let status = std::process::Command::new("curl")
        .arg("-fsSL")
        .arg("-o")
        .arg(dest)
        .arg(url)
        .status()
        .map_err(|e| exit::store_error(format!("running curl: {e}")))?;
    if !status.success() {
        // A failed fetch can leave a partial file, which would then block a
        // retry under the same name.
        let _ = std::fs::remove_file(dest);
        return Err(exit::not_found(format!("could not fetch {url} ({status})")));
    }
    Ok(())
}

/// The project's rows of one kind, newest first, each with its item: the
/// fields a list prints live in the file, not the index.
fn rows_of(app: &App, kind: Kind) -> Result<Vec<(Row, Item)>> {
    let identity = app.identity(Mode::Read)?;
    let index = app.read_index()?;
    let rows = index.recent_filtered(Some(kind.as_str()), None, identity.id(), ALL)?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let item = crate::store::read_item(&row.path).ok()?;
            Some((row, item))
        })
        .collect())
}

fn list(
    app: &App,
    rows: &[(Row, Item)],
    print: impl Fn(&Row, &Item) -> String,
    to_json: impl Fn(&Row, &Item) -> serde_json::Value,
) -> Result<i32> {
    if app.json {
        let items: Vec<serde_json::Value> = rows.iter().map(|(r, i)| to_json(r, i)).collect();
        println!("{}", serde_json::to_string(&json!({ "items": items }))?);
    } else {
        for (row, item) in rows {
            println!("{}", print(row, item));
        }
    }
    Ok(exit::OK)
}

fn first_line(item: &Item) -> String {
    item.body_str()
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string()
}

/// What `mem save` prints for the item it wrote.
fn report(app: &App, written: &Written, kind: Kind) -> Result<i32> {
    if app.json {
        println!(
            "{}",
            serde_json::to_string(&json!({
                "id": written.id,
                "short_id": written.short_id,
                "kind": kind.as_str(),
                "path": written.path.to_string_lossy(),
            }))?
        );
    } else if !app.quiet {
        println!("#{}  {}", written.short_id, kind);
    }
    Ok(exit::OK)
}
