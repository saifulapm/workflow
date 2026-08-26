//! The verbs themselves. Each returns the process exit code (spec §7), and
//! nothing here panics on an empty store: a fresh machine gets a helpful line,
//! not a stack trace.

use anyhow::Result;
use serde_json::json;

use crate::app::App;
use crate::digest::{Sources, TARGET};
use crate::exit;
use crate::ids::IdRef;
use crate::index::Row;
use crate::item::Item;
use crate::project::{Identity, Mode, Registry};
use crate::search::{Hit, Query, search};
use crate::timefmt::date;

/// `mem context` — the digest (spec §8). Always exit 0 when anything is
/// emitted, the empty state included: a hook that gets a non-zero exit here
/// would drop the whole thing.
pub fn context(app: &App, budget: Option<usize>, brief: bool, hook_json: bool) -> Result<i32> {
    let identity = app.identity(Mode::Read)?;
    let index = app.read_index()?;
    let staleness =
        crate::sync::staleness_line(&app.dirs.qshell_status_json(), jiff::Timestamp::now());
    let sources = Sources::gather(&index, &app.store, identity.id(), staleness)?;

    if brief {
        let text = crate::digest::brief(&sources, jiff::Timestamp::now());
        if hook_json {
            // The PostToolBatch hook: the envelope, on every fifth batch.
            return crate::hooks::post_tool_batch(app, &text);
        }
        if app.json {
            println!("{}", serde_json::to_string(&json!({ "brief": text }))?);
        } else if !text.is_empty() {
            println!("{text}");
        }
        return Ok(exit::OK);
    }

    let digest = crate::digest::build(&sources, &app.store, budget.unwrap_or(TARGET));
    if app.json {
        println!(
            "{}",
            serde_json::to_string(&json!({
                "context": digest.text,
                "truncated": digest.truncated,
                "project": identity.name(),
            }))?
        );
    } else {
        if let Some(note) = unknown_project_note(&identity)
            && !app.quiet
        {
            eprintln!("mem: {note}");
        }
        print!("{}", digest.text);
    }
    if digest.over_warn && !app.quiet {
        eprintln!(
            "mem: mandatory sections alone exceed {} bytes",
            crate::digest::WARN
        );
    }
    Ok(exit::OK)
}

/// `mem search "<q>"`.
pub fn search_verb(
    app: &App,
    text: &str,
    kind: Option<&str>,
    r#type: Option<&str>,
    limit: usize,
    min_score: Option<f64>,
) -> Result<i32> {
    let identity = app.identity(Mode::Read)?;
    let index = app.read_index()?;
    let query = Query {
        text,
        kind,
        r#type,
        limit,
        min_score,
        include_archived: app.include_archived,
        scope: app.search_scope(identity.id().map(|s| s.to_string())),
    };
    let hits = search(&index, &query)?;
    if app.json {
        println!(
            "{}",
            serde_json::to_string(&json!({ "hits": hits_json(&hits) }))?
        );
    } else {
        for hit in &hits {
            println!("{}", hit.line());
        }
    }
    if hits.is_empty() {
        if !app.quiet && !app.json {
            eprintln!("mem: nothing matched '{text}'");
        }
        return Ok(exit::NOT_FOUND);
    }
    Ok(exit::OK)
}

fn hits_json(hits: &[Hit]) -> Vec<serde_json::Value> {
    hits.iter()
        .map(|h| {
            let mut v = row_json(&h.row);
            v["score"] = json!((h.score * 100.0).round() / 100.0);
            v
        })
        .collect()
}

pub fn row_json(row: &Row) -> serde_json::Value {
    json!({
        "id": row.id,
        "short_id": row.short_id,
        "kind": row.kind,
        "type": row.r#type,
        "title": row.title,
        "tags": row.tags,
        "project": row.project,
        "machine": row.machine,
        "created": date(row.created_epoch),
        "modified": date(row.modified_epoch),
        "active": row.active,
        "archived": row.archived,
        "supersedes": row.supersedes,
        "superseded_by": row.superseded_by,
        "answers": row.answers,
        "path": row.path.to_string_lossy(),
    })
}

/// `mem show <id>...`. A short id that matches more than one item is ambiguous
/// rather than a guess: files are never renamed, so a suffix collision from a
/// sync merge is real and the full ULID is the only honest answer.
pub fn show(app: &App, ids: &[String]) -> Result<i32> {
    let index = app.read_index()?;
    let mut found: Vec<Row> = Vec::new();
    for raw in ids {
        let Some(id_ref) = IdRef::parse(raw) else {
            return Err(exit::not_found(format!(
                "'{raw}' is not an id — use a full ULID or its last 8 characters"
            )));
        };
        let mut rows = index.resolve_ref(&id_ref)?;
        match rows.len() {
            0 => return Err(exit::not_found(format!("no item {raw}"))),
            1 => found.push(rows.remove(0)),
            _ => {
                let candidates: Vec<String> = rows
                    .iter()
                    .map(|r| format!("  {}  {}  {}", r.id, r.kind, r.title))
                    .collect();
                return Err(exit::coded(
                    exit::AMBIGUOUS,
                    format!(
                        "'{raw}' is ambiguous — use the full ULID:\n{}",
                        candidates.join("\n")
                    ),
                ));
            }
        }
    }

    if app.json {
        let mut items = Vec::new();
        for row in &found {
            let mut v = row_json(row);
            v["body"] = json!(read_body(row));
            items.push(v);
        }
        println!("{}", serde_json::to_string(&json!({ "items": items }))?);
    } else {
        for (n, row) in found.iter().enumerate() {
            if n > 0 {
                println!();
            }
            // The file is the source of truth, so show it as it is on disk.
            match std::fs::read(&row.path) {
                Ok(bytes) => print!("{}", String::from_utf8_lossy(&bytes)),
                Err(_) => println!("#{}  {}  (file unreadable)", row.short_id, row.title),
            }
        }
    }
    Ok(exit::OK)
}

fn read_body(row: &Row) -> String {
    std::fs::read(&row.path)
        .ok()
        .and_then(|b| Item::parse(&b).ok())
        .map(|i| i.body_str().to_string())
        .unwrap_or_default()
}

/// `mem projects`.
pub fn projects(app: &App) -> Result<i32> {
    let registry = Registry::load(&app.store);
    let index = app.read_index()?;
    let identity = app.identity(Mode::Read)?;
    let current = identity.id();

    if app.json {
        let rows: Vec<serde_json::Value> = registry
            .projects
            .iter()
            .map(|p| {
                json!({
                    "id": p.id,
                    "name": p.name,
                    "remote": p.remote,
                    "aliases": p.aliases,
                    "created": p.created.to_string(),
                    "items": index.count_for_project(&p.id).unwrap_or(0),
                    "current": current == Some(p.id.as_str()),
                })
            })
            .collect();
        println!("{}", serde_json::to_string(&json!({ "projects": rows }))?);
        return Ok(exit::OK);
    }

    if registry.projects.is_empty() {
        println!("no projects yet — the first write in a git checkout registers one");
        return Ok(exit::OK);
    }
    for p in &registry.projects {
        let marker = if current == Some(p.id.as_str()) {
            "*"
        } else {
            " "
        };
        let count = index.count_for_project(&p.id).unwrap_or(0);
        println!(
            "{marker} {:<24} {:>5} items  {}",
            p.name,
            count,
            p.remote.as_deref().unwrap_or("")
        );
    }
    Ok(exit::OK)
}

/// `mem project current` — the sanctioned identity source for external tooling
/// (spec §7). It is a read verb, so an unregistered checkout is exit 1 and
/// nothing is created: whether a directory is a project mem knows is exactly
/// the question the workflow hooks ask.
pub fn project_current(app: &App) -> Result<i32> {
    let identity = app.identity(Mode::Read)?;
    let (Some(id), Some(name)) = (identity.id(), identity.name()) else {
        if !app.quiet {
            eprintln!(
                "mem: {}",
                unknown_project_note(&identity).unwrap_or_else(|| "no project here".to_string())
            );
        }
        return Ok(exit::NOT_FOUND);
    };
    // The root is this checkout, not a path recorded on some other machine.
    let root = crate::git::toplevel(&app.cwd);
    // Absent unless the project declared them: a caller that sees no `verify`
    // field falls through to its own detection (workflow spec §7, tier 1), and
    // one that sees no `review_paths` has only the global table.
    let declared = Registry::load(&app.store).by_id(id).cloned();
    let verify = declared.as_ref().and_then(|p| p.verify.clone());
    let review_paths = declared.as_ref().and_then(|p| p.review_paths.clone());
    // A child project keeps the checkout as its root — run dirs and worktrees
    // key on the checkout — and says where inside it the child lives.
    let subdir = declared.as_ref().and_then(|p| p.subdir.clone());
    if app.json {
        let mut doc = json!({
            "id": id,
            "name": name,
            "root": root.as_ref().map(|p| p.to_string_lossy()),
        });
        if let Some(subdir) = &subdir {
            doc["subdir"] = json!(subdir);
        }
        if let Some(verify) = &verify {
            doc["verify"] = json!(verify);
        }
        if let Some(paths) = &review_paths {
            doc["review_paths"] = json!(paths);
        }
        println!("{}", serde_json::to_string(&doc)?);
    } else {
        println!("id    {id}");
        println!("name  {name}");
        if let Some(root) = &root {
            println!("root  {}", root.display());
        }
        if let Some(subdir) = &subdir {
            println!("subdir  {subdir}");
        }
        if let Some(verify) = &verify {
            println!("verify  {verify}");
        }
        if let Some(paths) = &review_paths {
            println!("review-paths  {paths}");
        }
    }
    Ok(exit::OK)
}

/// `mem project add <subdir>` — a child project inside this checkout. The
/// root is resolved from the toplevel and registered first when the checkout
/// is new, so `add` works as the very first mem verb in a monorepo — and run
/// from inside one child it still hangs the new child off the root.
pub fn project_add(app: &App, subdir: &str, name: Option<&str>) -> Result<i32> {
    let Some(checkout) = crate::git::Checkout::detect(&app.cwd) else {
        return Err(exit::usage(
            "not a git checkout — a child project lives inside one".to_string(),
        ));
    };
    let identity = crate::project::resolve(
        &checkout.toplevel,
        &app.store,
        &app.dirs,
        None,
        Mode::Write,
    )?;
    let registry = Registry::load(&app.store);
    let root = identity
        .id()
        .and_then(|id| registry.by_id(id))
        .expect("a write in a checkout resolves to a project");
    let child =
        crate::project::register_child(&app.store, &registry, root, &checkout, subdir, name)?;
    if app.json {
        println!(
            "{}",
            serde_json::to_string(&json!({
                "id": child.id,
                "name": child.name,
                "subdir": child.subdir,
                "parent": root.id,
            }))?
        );
    } else if !app.quiet {
        println!(
            "{} ({}) registered under {}",
            child.name,
            child.subdir.as_deref().unwrap_or("?"),
            root.name
        );
    }
    Ok(exit::OK)
}

/// `mem project set <key> "<value>"` — the per-project verification command,
/// the per-project review paths. A write verb, so declaring one in a checkout
/// mem has never seen registers that checkout, exactly as `mem log` there
/// would.
pub fn project_set(app: &App, key: &str, value: &str) -> Result<i32> {
    let value = value.trim();
    if value.is_empty() {
        return Err(exit::usage(match key {
            "verify" => "give a command to run, e.g. `mem project set verify \"just test\"`"
                .to_string(),
            _ => format!(
                "give something to record, e.g. `mem project set {} \"app/**\"`",
                key.replace('_', "-")
            ),
        }));
    }
    let identity = app.identity(Mode::Write)?;
    let Some(id) = identity.id() else {
        return Err(exit::usage(format!(
            "{} — name one with --project",
            unknown_project_note(&identity).unwrap_or_else(|| "no project here".to_string())
        )));
    };
    let path = crate::project::set_key(&app.store, id, key, value)?;
    let shown = key.replace('_', "-");
    if app.json {
        println!(
            "{}",
            serde_json::to_string(&json!({
                "id": id,
                "name": identity.name(),
                key: value,
                "path": path.to_string_lossy(),
            }))?
        );
    } else if !app.quiet {
        println!("{shown} for {}: {value}", identity.name().unwrap_or(id));
    }
    Ok(exit::OK)
}

/// The one-line empty state a read verb prints when the working directory is
/// not a project mem knows (spec §5: reads never register).
pub fn unknown_project_note(identity: &Identity) -> Option<String> {
    match identity {
        Identity::UnknownRepo { name_hint } => Some(format!(
            "this checkout ({name_hint}) is not registered yet — a write registers it"
        )),
        Identity::NonGit => Some("not a git checkout — showing global scope only".to_string()),
        Identity::Known { .. } => None,
    }
}

/// `mem save "<text>"`.
pub fn save(
    app: &App,
    kind: &str,
    text: &str,
    title: Option<&str>,
    r#type: Option<&str>,
    tags: &[String],
    supersedes: Option<&str>,
) -> Result<i32> {
    let kind: crate::item::Kind = kind.parse().map_err(|e| exit::usage(format!("{e}")))?;
    let written = crate::write::save(app, kind, text, title, r#type, tags, supersedes)?;
    report_written(app, &written, kind)
}

fn report_written(
    app: &App,
    written: &crate::write::Written,
    kind: crate::item::Kind,
) -> Result<i32> {
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

/// `mem log` — dual mode: positional text writes, no text reads (spec §7).
pub fn log(
    app: &App,
    text: Option<&str>,
    limit: usize,
    since: Option<&str>,
    kind: Option<&str>,
    r#type: Option<&str>,
) -> Result<i32> {
    if let Some(text) = text {
        let written =
            crate::write::save(app, crate::item::Kind::Log, text, None, r#type, &[], None)?;
        return report_written(app, &written, crate::item::Kind::Log);
    }

    let identity = app.identity(Mode::Read)?;
    let index = app.read_index()?;
    let floor = match since {
        Some(s) => Some(crate::write::parse_since(s)?.as_second()),
        None => None,
    };
    let mut rows = index.recent(kind.unwrap_or("log"), identity.id(), limit.max(1) * 4)?;
    if let Some(floor) = floor {
        rows.retain(|r| r.modified_epoch >= floor);
    }
    if let Some(ty) = r#type {
        rows.retain(|r| r.r#type.as_deref() == Some(ty));
    }
    rows.truncate(limit);

    if app.json {
        let items: Vec<serde_json::Value> = rows.iter().map(row_json).collect();
        println!("{}", serde_json::to_string(&json!({ "items": items }))?);
    } else {
        for row in &rows {
            println!(
                "{}  #{}  {}",
                date(row.modified_epoch),
                row.short_id,
                row.title
            );
        }
    }
    if rows.is_empty() {
        return Ok(exit::NOT_FOUND);
    }
    Ok(exit::OK)
}

/// `mem handoff` — latest wins per project, and setting one asks for a sync.
pub fn handoff(app: &App, set: Option<&str>, stdin: bool, title: Option<&str>) -> Result<i32> {
    let text = match (set, stdin) {
        (Some(t), _) => Some(t.to_string()),
        (None, true) => Some(crate::write::read_stdin()?),
        (None, false) => None,
    };
    let Some(text) = text else {
        let identity = app.identity(Mode::Read)?;
        let index = app.read_index()?;
        let Some(row) = index
            .recent("handoff", identity.id(), 1)?
            .into_iter()
            .next()
        else {
            if !app.quiet {
                eprintln!("mem: no handoff recorded for this project");
            }
            return Ok(exit::NOT_FOUND);
        };
        if app.json {
            let mut v = row_json(&row);
            v["body"] = json!(read_body(&row));
            println!("{}", serde_json::to_string(&v)?);
        } else {
            print!("{}", read_body(&row));
        }
        return Ok(exit::OK);
    };

    let written = crate::write::save(
        app,
        crate::item::Kind::Handoff,
        &text,
        title,
        None,
        &[],
        None,
    )?;
    crate::sync::trigger(&app.dirs.qshell_status_json());
    report_written(app, &written, crate::item::Kind::Handoff)
}

/// `mem status` — prints status.md verbatim (a machine-readable contract), or
/// replaces it. An over-cap write still lands and reports exit 6.
pub fn status(app: &App, set: Option<&str>, stdin: bool) -> Result<i32> {
    if set.is_none() && !stdin {
        return print_singleton(app, |store, id| store.status_path(id));
    }
    let identity = app.identity(Mode::Write)?;
    let Some(id) = identity.id() else {
        return Err(exit::usage(
            "status belongs to a project — run this in a checkout or pass --project",
        ));
    };
    let path = app.store.status_path(id);
    // The CAS baseline is taken before the text is: `--stdin` blocks for as long
    // as the writer takes, and that whole window is when another machine's copy
    // can land (spec §4).
    let seen = crate::atomic::read_mtime(&path);
    let text = match set {
        Some(t) => t.to_string(),
        None => crate::write::read_stdin()?,
    };
    match crate::write::write_singleton_since(&path, &text, true, seen)? {
        crate::write::SingletonWrite::Conflict => Err(exit::coded(
            exit::CAS_CONFLICT,
            "status.md changed since it was read — re-read it and try again",
        )),
        crate::write::SingletonWrite::OverBudget { lines, bytes } => {
            self_record(app);
            eprintln!(
                "mem: status is on disk but over budget ({lines} lines, {bytes} bytes; \
                 cap is {} lines and {} bytes) — consolidate it",
                crate::write::STATUS_MAX_LINES,
                crate::write::STATUS_MAX_BYTES
            );
            Ok(exit::OVER_BUDGET)
        }
        crate::write::SingletonWrite::Written => {
            self_record(app);
            Ok(exit::OK)
        }
    }
}

fn self_record(app: &App) {
    if let Some(session) = &app.session_id {
        crate::session::record_write(&app.dirs.sessions_dir(), session);
    }
}

/// `mem plan` — prints plan.md verbatim, or replaces, clears or ticks it.
pub fn plan(
    app: &App,
    set_file: Option<&std::path::Path>,
    stdin: bool,
    clear: bool,
    tick: Option<&str>,
) -> Result<i32> {
    if let Some(task) = tick {
        return plan_tick(app, task);
    }
    if !clear && set_file.is_none() && !stdin {
        return print_singleton(app, |store, id| store.plan_path(id));
    }
    let identity = app.identity(Mode::Write)?;
    let Some(id) = identity.id() else {
        return Err(exit::usage(
            "a plan belongs to a project — run this in a checkout or pass --project",
        ));
    };
    let path = app.store.plan_path(id);
    if clear {
        match std::fs::remove_file(&path) {
            Ok(()) => {
                self_record(app);
                return Ok(exit::OK);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(exit::OK),
            Err(e) => return Err(exit::store_error(format!("clearing the plan: {e}"))),
        }
    }
    // As in `status`: the baseline is what the plan looked like before the
    // caller's text arrived, however long that took.
    let seen = crate::atomic::read_mtime(&path);
    let text = match set_file {
        Some(file) => std::fs::read_to_string(file)
            .map_err(|e| exit::not_found(format!("{}: {e}", file.display())))?,
        None => crate::write::read_stdin()?,
    };
    match crate::write::write_singleton_since(&path, &text, false, seen)? {
        crate::write::SingletonWrite::Conflict => Err(exit::coded(
            exit::CAS_CONFLICT,
            "plan.md changed since it was read — re-read it and try again",
        )),
        _ => {
            self_record(app);
            Ok(exit::OK)
        }
    }
}

/// `mem plan --tick <task-id>` — the CAS-safe checkbox tick (spec §7). The
/// project is resolved in read mode: a tick can only apply to a plan that
/// already exists, so there is never a project to invent here.
fn plan_tick(app: &App, task: &str) -> Result<i32> {
    let identity = app.identity(Mode::Read)?;
    let Some(id) = identity.id() else {
        return Err(exit::not_found("no plan here — this is not a mem project"));
    };
    let path = app.store.plan_path(id);
    let outcome = crate::write::tick_task(&path, task)?;
    let flipped = outcome == crate::write::Ticked::Flipped;
    if flipped {
        self_record(app);
    }
    if app.json {
        println!(
            "{}",
            serde_json::to_string(&json!({
                "id": task,
                "ticked": flipped,
                "path": path.to_string_lossy(),
            }))?
        );
    } else if !app.quiet {
        println!(
            "{}",
            if flipped {
                format!("- [x] {task}")
            } else {
                format!("- [x] {task} (already)")
            }
        );
    }
    Ok(exit::OK)
}

/// `mem wiki` — the project's pages, under `projects/<id>/wiki/`. Items are
/// episodic facts; a page is a document a session reads before it touches a
/// subsystem and updates when it changes one. There is no delete verb: bisync
/// resurrects deletions, so an obsolete page becomes a one-line stub pointing
/// at its replacement.
pub fn wiki(app: &App, slug: Option<&str>, stdin: bool, note: Option<&str>) -> Result<i32> {
    if let Some(slug) = slug
        && !crate::store::is_valid_slug(slug)
    {
        return Err(exit::usage(format!(
            "'{slug}' is not a page slug — lower case letters, digits and dashes, \
             starting with a letter or a digit, at most {} characters",
            crate::store::SLUG_MAX
        )));
    }
    if stdin || note.is_some() {
        let Some(slug) = slug else {
            return Err(exit::usage(
                "name the page to write, e.g. `mem wiki index --stdin --note \"why\"`",
            ));
        };
        return wiki_write(app, slug, stdin, note);
    }
    match slug {
        Some(slug) => wiki_print(app, slug),
        None => wiki_list(app),
    }
}

fn wiki_list(app: &App) -> Result<i32> {
    let identity = app.identity(Mode::Read)?;
    let pages = match identity.id() {
        Some(id) => app.store.wiki_pages(id),
        None => Vec::new(),
    };
    if app.json {
        let rows: Vec<serde_json::Value> = pages
            .iter()
            .map(|p| {
                json!({
                    "slug": p.slug,
                    "title": p.title,
                    "bytes": p.bytes,
                    "modified": date(p.modified_epoch),
                    "path": p.path.to_string_lossy(),
                })
            })
            .collect();
        println!("{}", serde_json::to_string(&json!({ "pages": rows }))?);
    } else {
        for p in &pages {
            println!(
                "{:<24} {:>6}  {}  {}",
                p.slug,
                p.bytes,
                date(p.modified_epoch),
                p.title
            );
        }
    }
    if pages.is_empty() {
        if !app.quiet && !app.json {
            eprintln!(
                "mem: {}",
                unknown_project_note(&identity).unwrap_or_else(|| {
                    "no pages yet — write one with `mem wiki <slug> --stdin --note \"why\"`"
                        .to_string()
                })
            );
        }
        return Ok(exit::NOT_FOUND);
    }
    Ok(exit::OK)
}

/// A page prints byte for byte, like plan.md and status.md: hub renders it and
/// a session reads it, and neither wants mem's opinion about markdown.
fn wiki_print(app: &App, slug: &str) -> Result<i32> {
    let identity = app.identity(Mode::Read)?;
    let bytes = identity
        .id()
        .map(|id| app.store.wiki_page(id, slug))
        .and_then(|path| std::fs::read(&path).ok().map(|bytes| (path, bytes)));
    let Some((path, bytes)) = bytes else {
        if !app.quiet && !app.json {
            eprintln!(
                "mem: {}",
                unknown_project_note(&identity)
                    .unwrap_or_else(|| format!("no page '{slug}' — `mem wiki` lists them"))
            );
        }
        return Ok(exit::NOT_FOUND);
    };
    if app.json {
        println!(
            "{}",
            serde_json::to_string(&json!({
                "slug": slug,
                "text": String::from_utf8_lossy(&bytes),
                "bytes": bytes.len(),
                "path": path.to_string_lossy(),
            }))?
        );
    } else {
        print!("{}", String::from_utf8_lossy(&bytes));
    }
    Ok(exit::OK)
}

fn wiki_write(app: &App, slug: &str, stdin: bool, note: Option<&str>) -> Result<i32> {
    if !stdin {
        return Err(exit::usage(
            "a note describes a write — add --stdin to replace the page",
        ));
    }
    // Both checks come before the project is resolved, so a refused write
    // registers nothing.
    let Some(note) = note.map(str::trim).filter(|n| !n.is_empty()) else {
        return Err(exit::usage(
            "a page write needs --note \"<what changed and why>\" — the note is the \
             page's history",
        ));
    };
    crate::maint::check_write_version(&app.store)?;
    let identity = app.identity(Mode::Write)?;
    let Some(id) = identity.id() else {
        return Err(exit::usage(
            "a page belongs to a project — run this in a checkout or pass --project",
        ));
    };
    let path = app.store.wiki_page(id, slug);
    // As in `status`: the CAS baseline is what the page looked like before the
    // writer's text arrived, and `--stdin` holds that window open for as long
    // as the writer takes.
    let seen = crate::atomic::read_mtime(&path);
    let text = crate::write::read_stdin()?;
    if text.trim().is_empty() {
        return Err(exit::usage(
            "a page needs text — there is no delete verb, so a page that is done \
             becomes a one-line stub pointing at what replaced it",
        ));
    }
    if let crate::write::SingletonWrite::Conflict =
        crate::write::write_singleton_since(&path, &text, false, seen)?
    {
        return Err(exit::coded(
            exit::CAS_CONFLICT,
            format!("{slug}.md changed since it was read — re-read it and try again"),
        ));
    }
    // History is the log line, not a revision of the file: one item per write,
    // typed so `mem log --type wiki` reads a wiki's whole story.
    let written = crate::write::save(
        app,
        crate::item::Kind::Log,
        &format!("wiki {slug}: {note}"),
        None,
        Some("wiki"),
        &[],
        None,
    )?;
    if app.json {
        println!(
            "{}",
            serde_json::to_string(&json!({
                "slug": slug,
                "path": path.to_string_lossy(),
                "bytes": std::fs::metadata(&path).map(|m| m.len()).unwrap_or_default(),
                "log": written.short_id,
            }))?
        );
    } else if !app.quiet {
        println!("wiki {slug}  #{}", written.short_id);
    }
    Ok(exit::OK)
}

/// plan.md and status.md are printed byte for byte: the workflow orchestrator
/// parses them, so this is a contract, not a display.
fn print_singleton(
    app: &App,
    which: fn(&crate::store::Store, &str) -> std::path::PathBuf,
) -> Result<i32> {
    let identity = app.identity(Mode::Read)?;
    let Some(id) = identity.id() else {
        if !app.quiet {
            eprintln!("mem: no project here");
        }
        return Ok(exit::NOT_FOUND);
    };
    let path = which(&app.store, id);
    match std::fs::read(&path) {
        Ok(bytes) => {
            if app.json {
                println!(
                    "{}",
                    serde_json::to_string(&json!({
                        "text": String::from_utf8_lossy(&bytes),
                        "path": path.to_string_lossy(),
                    }))?
                );
            } else {
                print!("{}", String::from_utf8_lossy(&bytes));
            }
            Ok(exit::OK)
        }
        Err(_) => {
            if !app.quiet && !app.json {
                eprintln!("mem: nothing recorded");
            }
            Ok(exit::NOT_FOUND)
        }
    }
}

/// `mem ask` — writes the question, asks for a sync, fires a notification and
/// returns. It never waits: a tool call that blocks for hours dies at the
/// runtime's ceiling, so waiting is `mem questions --wait`.
pub fn ask(app: &App, question: &str, options: &[String]) -> Result<i32> {
    let identity = app.identity(Mode::Write)?;
    let mut meta = crate::item::Meta::new(
        String::new(),
        crate::item::Kind::Question,
        crate::write::derive_title(question),
        app.machine.clone(),
    );
    if !options.is_empty() {
        meta.options = Some(options.to_vec());
    }
    let written = crate::write::write_item(app, &identity, meta, question.to_string())?;
    // No bell here: hub's doorbell owns delivery, and it knows whether anyone
    // is watching. When mem rang its own notify-send too, every question
    // arrived twice — and this one carried the question text, which the
    // doorbell deliberately never does.
    crate::sync::trigger(&app.dirs.qshell_status_json());
    if app.json {
        println!(
            "{}",
            serde_json::to_string(&json!({
                "id": written.id,
                "short_id": written.short_id,
                "options": options,
            }))?
        );
    } else {
        println!("#{}", written.short_id);
    }
    Ok(exit::OK)
}

/// `mem questions` and `mem questions --wait <id>`.
pub fn questions(
    app: &App,
    pending: bool,
    all_projects: bool,
    wait: Option<&str>,
    timeout: &str,
) -> Result<i32> {
    // A wait resolves its own scope from the question, so this checkout's
    // identity — and the git call behind it — is only the listing's business.
    if let Some(id) = wait {
        return wait_for(app, id, timeout);
    }

    let identity = app.identity(Mode::Read)?;
    let index = app.read_index()?;
    let rows = if all_projects {
        let mut all = index.pending_questions(None)?;
        for project in crate::project::Registry::load(&app.store).projects {
            all.extend(index.pending_questions(Some(&project.id))?);
        }
        all
    } else if pending {
        index.pending_questions(identity.id())?
    } else {
        index.recent("question", identity.id(), 50)?
    };

    if app.json {
        // The text mode has its ✓/? column; the JSON carries the same fact,
        // or a robot reading the recent listing cannot tell answered from
        // pending (found live, the first day a human answered one).
        let mut items: Vec<serde_json::Value> = Vec::with_capacity(rows.len());
        for row in &rows {
            let mut v = row_json(row);
            v["answered"] = json!(index.answer_to(&row.id)?.is_some());
            items.push(v);
        }
        println!("{}", serde_json::to_string(&json!({ "questions": items }))?);
    } else {
        for row in &rows {
            let answered = index.answer_to(&row.id)?.is_some();
            println!(
                "{} #{}  {}",
                if answered { "✓" } else { "?" },
                row.short_id,
                row.title
            );
        }
    }
    if rows.is_empty() {
        if !app.quiet && !app.json {
            eprintln!("mem: no questions");
        }
        return Ok(exit::NOT_FOUND);
    }
    Ok(exit::OK)
}

fn wait_for(app: &App, id: &str, timeout: &str) -> Result<i32> {
    let Some(timeout) = crate::questions::parse_timeout(timeout) else {
        return Err(exit::usage(format!(
            "'{timeout}' is not a duration — use 30s, 5m (5m is the maximum)"
        )));
    };
    let Some(id_ref) = crate::ids::IdRef::parse(id) else {
        return Err(exit::not_found(format!("'{id}' is not an id")));
    };
    let index = app.read_index()?;
    let mut rows = index.resolve_ref(&id_ref)?;
    let question = match rows.len() {
        0 => return Err(exit::not_found(format!("no question {id}"))),
        1 => rows.remove(0),
        _ => {
            return Err(exit::coded(
                exit::AMBIGUOUS,
                format!("'{id}' is ambiguous — use the full ULID"),
            ));
        }
    };
    drop(index);

    // The directory watched is the QUESTION's, not this checkout's: `mem
    // questions --wait` is routinely run from somewhere else entirely, and
    // stat-ing the wrong directory means never noticing the answer land.
    let items_dir = match question.project_id.as_deref() {
        Some(project_id) => app.store.project_items(project_id),
        None => app.store.global_items(),
    };
    let status_path = app.dirs.qshell_status_json();
    let outcome = crate::questions::wait_for_answer(
        &app.dirs.index_db(),
        &app.store,
        &items_dir,
        &question.id,
        timeout,
        || crate::sync::trigger(&status_path),
    )?;
    match outcome {
        crate::questions::Waited::Answered(answer) => {
            if app.json {
                let mut v = row_json(&answer);
                v["body"] = json!(read_body(&answer));
                println!("{}", serde_json::to_string(&v)?);
            } else {
                print!("{}", read_body(&answer));
            }
            Ok(exit::OK)
        }
        crate::questions::Waited::TimedOut => {
            if !app.quiet && !app.json {
                eprintln!(
                    "mem: #{} is still unanswered — park the work with `mem handoff`",
                    question.short_id
                );
            }
            Ok(exit::WAIT_TIMEOUT)
        }
    }
}

/// `mem answer <id> "<text>"`.
pub fn answer(app: &App, id: &str, text: Option<&str>, option: Option<&str>) -> Result<i32> {
    let Some(id_ref) = crate::ids::IdRef::parse(id) else {
        return Err(exit::not_found(format!("'{id}' is not an id")));
    };
    let index = app.read_index()?;
    let mut rows = index.resolve_ref(&id_ref)?;
    let question = match rows.len() {
        0 => return Err(exit::not_found(format!("no question {id}"))),
        1 => rows.remove(0),
        _ => {
            return Err(exit::coded(
                exit::AMBIGUOUS,
                format!("'{id}' is ambiguous — use the full ULID"),
            ));
        }
    };
    if question.kind != "question" {
        return Err(exit::not_found(format!(
            "#{} is a {}, not a question",
            question.short_id, question.kind
        )));
    }
    drop(index);

    let body = match (text, option) {
        (Some(t), _) => t.to_string(),
        (None, Some(o)) => o.to_string(),
        (None, None) => return Err(exit::usage("an answer needs text or --option")),
    };

    // The answer is written where its question lives, so both travel together.
    let identity = match question.project_id.as_deref() {
        Some(pid) => match crate::project::Registry::load(&app.store).by_id(pid) {
            Some(p) => Identity::Known {
                id: p.id.clone(),
                name: p.name.clone(),
            },
            None => Identity::NonGit,
        },
        None => Identity::NonGit,
    };
    let mut meta = crate::item::Meta::new(
        String::new(),
        crate::item::Kind::Answer,
        crate::write::derive_title(&body),
        app.machine.clone(),
    );
    meta.answers = Some(question.id.clone());
    let written = crate::write::write_item(app, &identity, meta, body)?;
    crate::sync::trigger(&app.dirs.qshell_status_json());
    report_written(app, &written, crate::item::Kind::Answer)
}

/// `mem reindex [--full]`.
pub fn reindex(app: &App, full: bool) -> Result<i32> {
    let index = crate::index::Index::open(&app.dirs.index_db(), crate::index::Purpose::Write)?;
    let outcome = index.reindex(&app.store, full)?;
    if app.json {
        println!(
            "{}",
            serde_json::to_string(&json!({
                "indexed": outcome.indexed,
                "deleted": outcome.deleted,
                "unreadable": outcome.unreadable,
                "skipped": outcome.skipped,
            }))?
        );
    } else if !app.quiet {
        // "indexed 0" reads as "there was nothing to do", which is the one
        // thing a skipped pass does not mean.
        if outcome.skipped {
            println!("skipped: another process holds the index");
        } else {
            println!(
                "indexed {}, removed {}, unreadable {}",
                outcome.indexed, outcome.deleted, outcome.unreadable
            );
        }
    }
    Ok(exit::OK)
}

/// `mem snapshot`.
pub fn snapshot(app: &App) -> Result<i32> {
    let path = crate::maint::snapshot(
        &app.store,
        &app.dirs.snapshots_dir(),
        jiff::Timestamp::now(),
    )?;
    if app.json {
        println!(
            "{}",
            serde_json::to_string(&json!({ "snapshot": path.to_string_lossy() }))?
        );
    } else if !app.quiet {
        println!("{}", path.display());
    }
    Ok(exit::OK)
}

/// `mem prune` — lists candidates; `--apply` archives them in place after a
/// snapshot, and never deletes anything.
pub fn prune(app: &App, apply: &[String]) -> Result<i32> {
    let index = app.read_index()?;
    let now = jiff::Timestamp::now();
    let candidates = crate::maint::prune_candidates(&index, now)?;

    if apply.is_empty() {
        if app.json {
            let rows: Vec<serde_json::Value> = candidates
                .iter()
                .map(|c| {
                    json!({"id": c.id, "short_id": c.short_id, "kind": c.kind,
                                 "title": c.title, "reason": c.reason})
                })
                .collect();
            println!("{}", serde_json::to_string(&json!({ "candidates": rows }))?);
        } else if candidates.is_empty() {
            println!("nothing stale");
        } else {
            for c in &candidates {
                println!("#{}  {}  {}  ({})", c.short_id, c.kind, c.title, c.reason);
            }
            println!("archive them with: mem prune --apply <id>...  (or --apply all)");
        }
        return Ok(exit::OK);
    }

    crate::maint::check_write_version(&app.store)?;
    // The undo for a prune that went too far, taken before anything changes.
    crate::maint::snapshot(&app.store, &app.dirs.snapshots_dir(), now)?;
    let all = apply.iter().any(|a| a == "all");
    let mut archived = Vec::new();
    let mut resolved: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for c in &candidates {
        let asked = apply
            .iter()
            .find(|a| a.eq_ignore_ascii_case(&c.short_id) || **a == c.id);
        if let Some(a) = asked {
            resolved.insert(a.as_str());
        }
        if all || asked.is_some() {
            crate::maint::archive_in_place(&c.path, now)?;
            archived.push(c.short_id.clone());
        }
    }
    if app.json {
        println!(
            "{}",
            serde_json::to_string(&json!({ "archived": archived }))?
        );
    } else if !app.quiet {
        println!("archived {} item(s) in place", archived.len());
    }

    // An id that matched no candidate used to archive nothing and say nothing,
    // which leaves the caller believing it archived something. The report above
    // still stands — what did land, landed — and the exit code carries the rest.
    let unknown: Vec<&str> = if all {
        Vec::new()
    } else {
        apply
            .iter()
            .map(|a| a.as_str())
            .filter(|a| !resolved.contains(a))
            .collect()
    };
    if !unknown.is_empty() {
        if !app.quiet {
            eprintln!(
                "mem: nothing stale to archive for {} — `mem prune` lists the candidates, \
                 and `mem prune --apply all` takes all of them",
                unknown.join(", ")
            );
        }
        return Ok(exit::NOT_FOUND);
    }
    Ok(exit::OK)
}

/// `mem sync` — asks qshell-sync for a round and verifies that one happened.
pub fn sync(app: &App) -> Result<i32> {
    let outcome = crate::sync::verified(
        &app.dirs.qshell_status_json(),
        std::time::Duration::from_secs(15),
    )?;
    let (state, detail) = match &outcome {
        crate::sync::Outcome::Performed { detail } => ("performed", detail.clone()),
        crate::sync::Outcome::NotPerformed { reason } => ("not performed", reason.clone()),
        crate::sync::Outcome::Deferred { reason } => ("deferred", reason.clone()),
    };
    if app.json {
        println!(
            "{}",
            serde_json::to_string(&json!({ "sync": state, "detail": detail }))?
        );
    } else if !app.quiet {
        println!("sync {state}: {detail}");
    }
    Ok(exit::OK)
}

/// The page every wiki has: one line per page, written by whoever writes the
/// pages. Doctor is what keeps it honest.
const WIKI_INDEX: &str = "index";

/// Past this a page is a page to compact. There is no hard cap — a wiki grows
/// by being written to, and compaction is a verb, not a refusal.
const WIKI_PAGE_WARN_BYTES: u64 = 8 * 1024;

/// The slug a markdown link points at, when it points at a page in the same
/// wiki. `[name](name.md)` is the whole convention: an anchor is trimmed, and a
/// target with a scheme or a slash in it lives in some other tree.
fn page_link_target(target: &str) -> Option<&str> {
    let target = target.split('#').next().unwrap_or_default();
    if target.contains('/') || target.contains(':') {
        return None;
    }
    let slug = target.strip_suffix(".md")?;
    crate::store::is_valid_slug(slug).then_some(slug)
}

/// The target of every `[text](target)` in a page. Enough markdown to find the
/// links the wiki convention writes, and no more: a title after the target is
/// dropped, and a link mem cannot recognise is left for the renderer.
fn markdown_links(text: &str) -> Vec<&str> {
    let mut targets = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find("](") {
        rest = &rest[open + 2..];
        let Some(close) = rest.find(')') else { break };
        let target = &rest[..close];
        rest = &rest[close + 1..];
        targets.push(target.split_whitespace().next().unwrap_or_default());
    }
    targets
}

/// The wiki checks: links that point at no page, drift between the index page
/// and the directory in both directions, pages worth compacting, and the
/// secrets grep the item files already get.
fn wiki_findings(app: &App, project: &crate::project::Project) -> Vec<crate::maint::Finding> {
    use crate::maint::{finding, looks_like_a_secret};
    let mut findings = Vec::new();
    let pages = app.store.wiki_pages(&project.id);
    if pages.is_empty() {
        return findings;
    }
    let slugs: std::collections::HashSet<&str> = pages.iter().map(|p| p.slug.as_str()).collect();
    let mut listed: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut has_index = false;

    for page in &pages {
        let name = format!("{}/{}.md", project.name, page.slug);
        if page.bytes > WIKI_PAGE_WARN_BYTES {
            findings.push(finding(
                "wiki size",
                format!(
                    "{name} is {} KB — compact it, or split it and link the parts",
                    page.bytes / 1024
                ),
            ));
        }
        let Ok(text) = std::fs::read_to_string(&page.path) else {
            findings.push(finding(
                "unreadable",
                format!("{} is in no read", page.path.display()),
            ));
            continue;
        };
        // Per line, so the report names where to look: a page is a document,
        // and "somewhere in here" is not a place.
        for (n, line) in text.lines().enumerate() {
            if let Some(shape) = looks_like_a_secret(line) {
                findings.push(finding(
                    "secret",
                    format!("{name} line {} looks like it contains {shape}", n + 1),
                ));
            }
        }
        let is_index = page.slug == WIKI_INDEX;
        has_index |= is_index;
        for target in markdown_links(&text) {
            let Some(slug) = page_link_target(target) else {
                continue;
            };
            if is_index {
                // A dangling link from the index is drift, not a broken link:
                // one problem, one finding.
                listed.insert(slug.to_string());
                if !slugs.contains(slug) {
                    findings.push(finding(
                        "wiki index",
                        format!(
                            "{}'s index lists {slug}.md, which is not a page",
                            project.name
                        ),
                    ));
                }
            } else if !slugs.contains(slug) {
                findings.push(finding(
                    "wiki link",
                    format!("{name} links to {slug}.md, which is not a page"),
                ));
            }
        }
    }

    if !has_index {
        // Every page is missing from an index that does not exist, and saying
        // so once is the finding. Listing them one by one buries it.
        findings.push(finding(
            "wiki index",
            format!(
                "{} has {} page(s) and no index page — write one, a line per page",
                project.name,
                pages.len()
            ),
        ));
        return findings;
    }
    for page in &pages {
        if page.slug != WIKI_INDEX && !listed.contains(page.slug.as_str()) {
            findings.push(finding(
                "wiki index",
                format!("{}/{}.md is not in the index", project.name, page.slug),
            ));
        }
    }
    findings
}

/// `mem doctor [--fix]` — every check in spec §7. Findings are exit 0: they are
/// for a human to read, not a failure of the command.
pub fn doctor(app: &App, fix: bool) -> Result<i32> {
    use crate::maint::{finding, looks_like_a_secret};
    let mut findings = Vec::new();
    let index = app.read_index()?;

    if let Some(warning) = crate::maint::read_version_warning(&app.store) {
        findings.push(finding("version", warning));
    }
    match crate::sync::Status::read(&app.dirs.qshell_status_json()) {
        Some(status) => match status.unit(crate::sync::UNIT) {
            Some(unit) if unit.ok == Some(false) => findings.push(finding(
                "sync",
                format!(
                    "the memory unit is failing: {} {}",
                    unit.error_kind, unit.error
                ),
            )),
            Some(unit) => {
                if let Some(line) = status.staleness_warning(jiff::Timestamp::now()) {
                    findings.push(finding("sync", line));
                } else if unit.last_ok.is_empty() {
                    findings.push(finding("sync", "the memory unit has never synced"));
                }
            }
            None => findings.push(finding(
                "sync",
                "qshell-sync has no memory unit yet — see mem/TESTING.md for the unit block",
            )),
        },
        None => findings.push(finding(
            "sync",
            "no qshell-sync status file on this machine",
        )),
    }

    for stray in app.store.stray_paths() {
        let name = stray
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let check = if name.ends_with(".path1") || name.ends_with(".path2") {
            "conflict"
        } else if name.starts_with(".tmp-") {
            "temp"
        } else {
            "stray"
        };
        findings.push(finding(check, stray.to_string_lossy().to_string()));
    }

    let backlog = crate::maint::outbox_backlog(&app.dirs.outbox_dir());
    if !backlog.is_empty() {
        if fix {
            let replayed = crate::maint::replay_outbox(&app.store, &app.dirs.outbox_dir())?;
            findings.push(finding(
                "outbox",
                format!("replayed {replayed} spooled write(s)"),
            ));
        } else {
            findings.push(finding(
                "outbox",
                format!(
                    "{} spooled write(s) waiting — mem doctor --fix replays them",
                    backlog.len()
                ),
            ));
        }
    }

    for (target, ids) in index.supersede_forks()? {
        findings.push(finding(
            "supersede fork",
            format!("{} is superseded by {}", target, ids.join(" and ")),
        ));
    }

    // Two files whose ids share a suffix can only come from a sync merge, and
    // they can never be renamed apart.
    let mut by_short: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for path in app.store.item_paths() {
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            by_short
                .entry(crate::ids::short_id(stem))
                .or_default()
                .push(stem.to_string());
        }
    }
    let mut collisions: Vec<String> = by_short
        .into_iter()
        .filter(|(_, ids)| ids.len() > 1)
        .map(|(short, ids)| format!("{short}: {}", ids.join(" ")))
        .collect();
    collisions.sort();
    for c in collisions {
        findings.push(finding("short id collision", c));
    }

    let registry = crate::project::Registry::load(&app.store);
    for p in &registry.projects {
        for alias in &p.aliases {
            if registry.name_taken(alias) {
                findings.push(finding(
                    "name collision",
                    format!(
                        "{} wanted the name '{alias}', which another project has",
                        p.name
                    ),
                ));
            }
        }
        let status_path = app.store.status_path(&p.id);
        if let Ok(text) = std::fs::read_to_string(&status_path)
            && (text.lines().count() > crate::write::STATUS_MAX_LINES
                || text.len() > crate::write::STATUS_MAX_BYTES)
        {
            findings.push(finding(
                "budget",
                format!("{}'s status.md is over cap — consolidate it", p.name),
            ));
        }
        findings.extend(wiki_findings(app, p));
    }

    for path in app.store.item_paths() {
        match crate::store::read_item(&path) {
            Ok(item) => {
                if let Some(shape) = looks_like_a_secret(&item.body_str()) {
                    findings.push(finding(
                        "secret",
                        format!("#{} looks like it contains {shape}", item.meta.short_id()),
                    ));
                }
            }
            // The reindex counts these and carries on, which is right — one
            // mangled file must not fail the pass. But a file nothing can parse
            // is in no search result and no digest, and in a system whose pillar
            // is "files are source of truth" that has to be said out loud.
            Err(e) => findings.push(finding(
                "unreadable",
                format!("{} is in no read: {}", path.display(), e.root_cause()),
            )),
        }
    }

    let cleaned = crate::session::cleanup(&app.dirs.sessions_dir());
    if cleaned > 0 {
        findings.push(finding(
            "sessions",
            format!("removed {cleaned} stale session file(s)"),
        ));
    }

    if app.json {
        println!(
            "{}",
            serde_json::to_string(&json!({ "findings": findings }))?
        );
    } else if findings.is_empty() {
        println!("no findings");
    } else {
        for f in &findings {
            println!("{:<20} {}", f.check, f.detail);
        }
    }
    Ok(exit::OK)
}
