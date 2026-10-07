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
use crate::project::{Identity, Mode, PathMap, Registry};
use crate::search::{Hit, Query, search};
use crate::timefmt::date;

/// `mem context` — the small digest, or with `--full` the whole one (spec §8).
/// Always exit 0 when anything is
/// emitted, the empty state included: a hook that gets a non-zero exit here
/// would drop the whole thing.
///
/// Outside a project mem knows there is no digest to print. Registration is the
/// adapter's whole gate: a checkout mem has never been told about gets silence
/// rather than two lines saying so, because those two lines are injected into a
/// session that then has to decide what to do about them. `--json` still prints
/// its document — machines read that, not the adapter.
pub fn context(
    app: &App,
    full: bool,
    budget: Option<usize>,
    brief: bool,
    hook_json: bool,
) -> Result<i32> {
    let identity = app.identity(Mode::Read)?;
    if !app.json && !matches!(identity, Identity::Known { .. }) {
        if let Some(note) = unknown_project_note(&identity)
            && !app.quiet
        {
            eprintln!("mem: {note}");
        }
        return Ok(exit::OK);
    }
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
            return Err(exit::usage(
                "--brief prints a hook's text and has no --json form; \
                 `mem context --json` is the digest as JSON",
            ));
        }
        if !text.is_empty() {
            println!("{text}");
        }
        return Ok(exit::OK);
    }

    let digest = if full {
        crate::digest::build(&sources, &app.store, budget.unwrap_or(TARGET))
    } else {
        crate::digest::build_small(&sources, &app.store)
    };
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
        // Named first: a session that inherited another's MEM_PROJECT read
        // amx's memory in shortcart's checkout and could not tell.
        // A warning about the store still leads. The small digest names the
        // project itself.
        let at = if full {
            digest
                .text
                .split_inclusive('\n')
                .take_while(|l| l.starts_with("! "))
                .map(str::len)
                .sum::<usize>()
        } else {
            0
        };
        print!("{}", &digest.text[..at]);
        if full && let Some(name) = identity.name() {
            println!("project: {name}");
        }
        print!("{}", &digest.text[at..]);
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
            serde_json::to_string(&json!({ "hits": hits_json(&hits, kind.is_some()) }))?
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

/// A search narrowed to one kind is a listing a page renders whole, so its
/// item rows carry their text; a page section has its snippet instead.
fn hits_json(hits: &[Hit], bodies: bool) -> Vec<serde_json::Value> {
    hits.iter()
        .map(|h| {
            let mut v = row_json(&h.row);
            v["score"] = json!((h.score * 100.0).round() / 100.0);
            if bodies && h.row.kind != crate::index::WIKI_KIND {
                v["body"] = json!(read_body(&h.row));
            }
            if let Some(heading) = &h.heading {
                v["heading"] = json!(heading);
                v["snippet"] = json!(h.snippet);
                v["bytes"] = json!(h.bytes);
            }
            v
        })
        .collect()
}

/// `by` and `replaces` are read from the item's file: the index keeps neither,
/// and a reader of rulings wants both on the row.
pub fn row_json(row: &Row) -> serde_json::Value {
    let item = read_item(row);
    let meta = item.as_ref().map(|i| &i.meta);
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
        "audience": row.audience,
        "task": row.task,
        "by": meta.and_then(|m| m.by.as_deref()),
        "replaces": meta.and_then(|m| m.replaces.as_deref()),
        "about": meta.and_then(|m| m.about.as_deref()),
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
    read_item(row)
        .map(|i| i.body_str().to_string())
        .unwrap_or_default()
}

fn read_item(row: &Row) -> Option<Item> {
    std::fs::read(&row.path)
        .ok()
        .and_then(|b| Item::parse(&b).ok())
}

/// `mem projects`.
pub fn projects(app: &App) -> Result<i32> {
    let registry = Registry::load(&app.store);
    let index = app.read_index()?;
    let identity = app.identity(Mode::Read)?;
    let current = identity.id();

    if app.json {
        let path_map = PathMap::load(&app.dirs.paths_toml());
        let rows: Vec<serde_json::Value> = registry
            .projects
            .iter()
            .map(|p| {
                let root_id = p.parent.as_deref().unwrap_or(&p.id);
                let checkouts: Vec<String> = path_map
                    .roots(root_id)
                    .into_iter()
                    .map(|root| match &p.subdir {
                        Some(subdir) => root.join(subdir).to_string_lossy().to_string(),
                        None => root.to_string_lossy().to_string(),
                    })
                    .collect();
                let mut row = json!({
                    "id": p.id,
                    "name": p.name,
                    "remote": p.remote,
                    "aliases": p.aliases,
                    "created": p.created.to_string(),
                    "items": index.count_for_project(&p.id).unwrap_or(0),
                    "current": current == Some(p.id.as_str()),
                    "checkouts": checkouts,
                });
                for (key, value) in project_summary(app, &index, &p.id) {
                    row[key] = value;
                }
                row
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

/// What a project page shows of one project, from project.toml, the two
/// singletons, the wiki directory and the index. No item file is opened: this
/// runs for every project on every read of the project list.
fn project_summary(
    app: &App,
    index: &crate::index::Index,
    id: &str,
) -> Vec<(&'static str, serde_json::Value)> {
    let declared = |key| crate::project::declared(&app.store, id, key);
    let read = |path: std::path::PathBuf| std::fs::read_to_string(path).unwrap_or_default();
    let roadmap_text = read(app.store.roadmap_path(id));
    let roadmap = checkboxes(&roadmap_text);
    let plan_text = read(app.store.plan_path(id));
    let plan = checkboxes(&plan_text);
    let plan_slug = plan_text
        .lines()
        .next()
        .and_then(|first| header_slug(first, "plan"));
    let last_activity = index
        .recent_filtered(None, None, Some(id), 1)
        .ok()
        .and_then(|rows| rows.into_iter().next())
        .and_then(|row| jiff::Timestamp::from_second(row.modified_epoch).ok())
        .map(|ts| ts.to_string());
    let pages = app.store.wiki_pages(id);
    let has_page = |wanted: fn(&str) -> bool| pages.iter().any(|page| wanted(&page.slug));
    vec![
        ("roadmap_status", json!(declared(ROADMAP_STATUS))),
        (
            "milestone",
            json!(roadmap.iter().find(|(_, done)| !done).map(|(slug, _)| slug)),
        ),
        (
            "milestones_done",
            json!(roadmap.iter().filter(|(_, done)| *done).count()),
        ),
        ("milestones_total", json!(roadmap.len())),
        ("plan_slug", json!(plan_slug)),
        (
            "plan_ticked",
            json!(plan.iter().filter(|(_, done)| *done).count()),
        ),
        ("plan_total", json!(plan.len())),
        ("last_activity", json!(last_activity)),
        (
            "has_research",
            json!(has_page(|s| s.starts_with("research"))),
        ),
        (
            "has_research_summary",
            json!(has_page(|s| s == "research-summary")),
        ),
        ("has_spec", json!(has_page(|s| s == "spec"))),
    ]
}

/// The `- [ ] <id>` and `- [x] <id>` lines of a plan or a roadmap, as the id
/// and whether it is ticked, in order.
fn checkboxes(text: &str) -> Vec<(&str, bool)> {
    text.lines()
        .filter_map(|l| {
            let l = l.trim_start();
            let (rest, done) = if let Some(rest) = l.strip_prefix("- [ ] ") {
                (rest, false)
            } else {
                (l.strip_prefix("- [x] ").or(l.strip_prefix("- [X] "))?, true)
            };
            Some((rest.split_whitespace().next()?, done))
        })
        .collect()
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
    // field falls through to its own detection.
    let declared = Registry::load(&app.store).by_id(id).cloned();
    let verify = declared.as_ref().and_then(|p| p.verify.clone());
    let hygiene_exempt = crate::project::declared(&app.store, id, "hygiene_exempt");
    // A child project keeps the checkout as its root and says where inside
    // it the child lives.
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
        if let Some(globs) = &hygiene_exempt {
            doc["hygiene_exempt"] = json!(globs);
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
        if let Some(globs) = &hygiene_exempt {
            println!("hygiene-exempt  {globs}");
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
    let identity =
        crate::project::resolve(&checkout.toplevel, &app.store, &app.dirs, None, Mode::Write)?;
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
/// the hygiene exemptions, the remote. Unlike other write
/// verbs, `project set` and `project unset` configure a project rather than
/// record against one, so they resolve in `Mode::Read` and refuse an
/// unregistered checkout instead of registering it; `claim_note` only
/// sharpens the refusal's message when a project already owns the name.
pub fn project_set(app: &App, key: &str, value: &str) -> Result<i32> {
    let value = value.trim();
    if value.is_empty() {
        return Err(exit::usage(match key {
            "verify" => {
                "give a command to run, e.g. `mem project set verify \"just test\"`".to_string()
            }
            "remote" => {
                "give a url, e.g. `mem project set remote git@github.com:acme/app.git`".to_string()
            }
            _ => format!(
                "give something to record, e.g. `mem project set {} \"app/**\"`",
                key.replace('_', "-")
            ),
        }));
    }
    // A remote is an identity, not free text: store the spelling registration
    // would have, so `by_remote` finds this project from another machine.
    let normalized;
    let value = if key == "remote" {
        normalized = crate::git::normalize_remote(value);
        normalized.as_str()
    } else {
        value
    };
    let identity = writable_project_identity(app)?;
    let Some(id) = identity.id() else {
        return Err(exit::usage(unregistered_project_note(app, &identity)));
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

/// `mem project unset <key>` -- the way back to absent. `set` refuses an
/// empty value, so without this the only way to take a key off a project
/// was to edit project.toml by hand.
pub fn project_unset(app: &App, key: &str) -> Result<i32> {
    let identity = writable_project_identity(app)?;
    let Some(id) = identity.id() else {
        return Err(exit::usage(unregistered_project_note(app, &identity)));
    };
    let (path, had) = crate::project::unset_key(&app.store, id, key)?;
    let shown = key.replace('_', "-");
    if app.json {
        println!(
            "{}",
            serde_json::to_string(&json!({
                "id": id,
                "name": identity.name(),
                "unset": key,
                "was_set": had,
                "path": path.to_string_lossy(),
            }))?
        );
    } else if !app.quiet {
        println!(
            "{shown} for {}: {}",
            identity.name().unwrap_or(id),
            if had { "cleared" } else { "was not set" }
        );
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

/// A checkout `project set`/`project unset` does not know might still be a
/// project mem does: the poshra case, a project registered from one checkout
/// with a second checkout of it that has never been recorded — by remote or
/// by path — against it. `Some` names the project that already owns the name
/// and the `--project` form that reaches it, diagnosing a missing remote
/// where that is why the checkout's own remote never matched it, or names an
/// alias collision the registry refused to pick between; `None` when no
/// project shares the name, so the caller falls back to its own generic note.
/// A child project is left alone: it shares its root's remote, so
/// pointing an unrelated checkout at it by remote would fold that checkout's
/// notes into the root's child instead of a project of its own.
pub fn claim_note(registry: &Registry, name_hint: &str) -> Option<String> {
    match registry.by_name(name_hint) {
        Ok(Some(project)) if project.parent.is_some() => None,
        Ok(Some(project)) if project.remote.is_none() => Some(format!(
            "this checkout ({name_hint}) is not registered, and a project named '{}' exists \
             without a remote — `mem project set --project {} remote <url>` sets it there, and \
             the next verb here finds it by that remote",
            project.name, project.name
        )),
        Ok(Some(project)) => Some(format!(
            "this checkout ({name_hint}) is not registered, and a project named '{}' already \
             exists — set it explicitly with --project {}",
            project.name, project.name
        )),
        Ok(None) => None,
        Err(err) => Some(format!(
            "this checkout ({name_hint}) is not registered, and {err}"
        )),
    }
}

/// The identity `project set` and `project unset` write against. Unlike
/// every other write verb, these two never auto-register: they configure a
/// project, so the project has to exist first, resolved in Read mode only.
/// `--project <name>` still names an existing one to write.
fn writable_project_identity(app: &App) -> Result<Identity> {
    app.identity(Mode::Read)
}

/// The usage error `project set` and `project unset` give when the checkout
/// they ran in is not a registered project: the name hint and, when a
/// project already owns that name, the claim naming it.
fn unregistered_project_note(app: &App, identity: &Identity) -> String {
    match identity {
        Identity::UnknownRepo { name_hint } => claim_note(&Registry::load(&app.store), name_hint)
            .unwrap_or_else(|| {
                format!(
                    "this checkout ({name_hint}) is not registered — name a project with --project"
                )
            }),
        Identity::NonGit => "not a git checkout — name a project with --project".to_string(),
        Identity::Known { .. } => "no project here — name one with --project".to_string(),
    }
}

/// What `mem save` was told about the item besides its text.
pub struct SaveMeta<'a> {
    pub title: Option<&'a str>,
    pub r#type: Option<&'a str>,
    pub tags: &'a [String],
    pub supersedes: Option<&'a str>,
    pub about: Option<&'a str>,
}

/// `mem save "<text>"`.
pub fn save(app: &App, kind: &str, text: &str, with: SaveMeta) -> Result<i32> {
    let kind: crate::item::Kind = kind.parse().map_err(|e| exit::usage(format!("{e}")))?;
    let (identity, mut meta) = crate::write::new_meta(
        app,
        kind,
        text,
        with.title,
        with.r#type,
        with.tags,
        with.supersedes,
    )?;
    meta.about = with.about.map(str::to_string);
    let written = crate::write::write_item(app, &identity, meta, text.to_string())?;
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
/// `bodies` puts each row's text on its JSON, for `mem search --kind`.
pub fn log(
    app: &App,
    text: Option<&str>,
    limit: usize,
    since: Option<&str>,
    kind: Option<&str>,
    r#type: Option<&str>,
    bodies: bool,
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
    // `log` is the default kind only while nothing else narrows the read: a
    // `--type` on its own means that type in every kind, because a follow-up
    // is a fact and `mem log --type followup` is the hint a run prints.
    let kind = kind.or_else(|| r#type.is_none().then_some("log"));
    let mut rows = index.recent_filtered(kind, r#type, identity.id(), limit.max(1))?;
    if let Some(floor) = floor {
        rows.retain(|r| r.modified_epoch >= floor);
    }

    if app.json {
        let items: Vec<serde_json::Value> = rows
            .iter()
            .map(|row| {
                let mut v = row_json(row);
                if bodies {
                    v["body"] = json!(read_body(row));
                }
                v
            })
            .collect();
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

fn self_record(app: &App) {
    if let Some(session) = &app.session_id {
        crate::session::record_write(&app.dirs.sessions_dir(), session);
    }
}

/// The plan singletons: the current plan, and the roadmap of milestones
/// above it. One grammar and one set of verbs over two files, so the handling
/// is written once and told which file it is acting on.
#[derive(Clone, Copy)]
struct Singleton {
    /// What it is, in the sentence "a plan belongs to a project".
    noun: &'static str,
    /// What one line of it ticks: "task" for the plan, "milestone" for the
    /// roadmap.
    item: &'static str,
    /// What the file is called, for the messages that name it.
    file: &'static str,
    /// The project.toml key its status lives under, for the roadmap. The text
    /// is parsed by more readers than mem, so the status stays out of it.
    status_key: Option<&'static str>,
    path: fn(&crate::store::Store, &str) -> std::path::PathBuf,
}

const PLAN: Singleton = Singleton {
    noun: "plan",
    item: "task",
    file: "plan.md",
    status_key: None,
    path: |store, id| store.plan_path(id),
};

/// Where the roadmap's status lives in project.toml.
const ROADMAP_STATUS: &str = "roadmap_status";

const ROADMAP: Singleton = Singleton {
    noun: "roadmap",
    item: "milestone",
    file: "roadmap.md",
    status_key: Some(ROADMAP_STATUS),
    path: |store, id| store.roadmap_path(id),
};

/// What `mem plan` was asked for. The verb reaches from the current plan to
/// the stored milestone plans, which is more than a row of positional flags
/// reads well as.
pub struct PlanArgs<'a> {
    pub slug: Option<&'a str>,
    pub set_file: Option<&'a std::path::Path>,
    pub stdin: bool,
    pub clear: bool,
    pub tick: Option<&'a str>,
    pub list: bool,
}

/// `mem plan` — the current plan and the milestone plans stored beside it.
pub fn plan(app: &App, args: PlanArgs<'_>) -> Result<i32> {
    if args.list {
        return list_slug_files(
            app,
            |store, id| store.stored_plans(id),
            "plans",
            "no stored plans — write one with `mem plan <slug> --set-file <file>`",
        );
    }
    let Some(slug) = args.slug else {
        return match args.tick {
            Some(task) => tick_singleton(app, PLAN, task),
            None => singleton(app, PLAN, args.set_file, args.stdin, args.clear),
        };
    };
    check_slug(slug, "plan")?;
    if args.tick.is_some() {
        return Err(exit::usage(
            "a tick belongs to the current plan — `mem plan --tick <task-id>`",
        ));
    }
    stored_plan(app, slug, args.set_file, args.stdin, args.clear)
}

/// What `mem roadmap` was asked for.
pub struct RoadmapArgs<'a> {
    pub set_file: Option<&'a std::path::Path>,
    pub stdin: bool,
    pub clear: bool,
    pub tick: Option<&'a str>,
    /// `--status` alone prints it; with a value, sets it.
    pub status: Option<Option<&'a str>>,
}

/// `mem roadmap` — the same moves over roadmap.md, and its status.
pub fn roadmap(app: &App, args: RoadmapArgs<'_>) -> Result<i32> {
    if let Some(status) = args.status {
        return roadmap_status(app, status);
    }
    match args.tick {
        Some(slug) => tick_singleton(app, ROADMAP, slug),
        None => singleton(app, ROADMAP, args.set_file, args.stdin, args.clear),
    }
}

/// What `mem roadmap --status` sets. A store synced from an older mem can
/// still hold another word, which reads back as it was written.
const STATUSES: [&str; 2] = ["draft", "approved"];

/// `mem roadmap --status`: print the status, or set it in project.toml.
fn roadmap_status(app: &App, status: Option<&str>) -> Result<i32> {
    let Some(status) = status else {
        let identity = app.identity(Mode::Read)?;
        let Some(value) = identity
            .id()
            .and_then(|id| crate::project::declared(&app.store, id, ROADMAP_STATUS))
        else {
            if !app.quiet && !app.json {
                eprintln!(
                    "mem: no roadmap status set — `mem roadmap --status <{}>` sets one",
                    STATUSES.join("|")
                );
            }
            return Ok(exit::NOT_FOUND);
        };
        if app.json {
            println!("{}", serde_json::to_string(&json!({ "status": value }))?);
        } else {
            println!("{value}");
        }
        return Ok(exit::OK);
    };
    if !STATUSES.contains(&status) {
        return Err(exit::usage(format!(
            "a roadmap status is one of {}, not `{status}`",
            STATUSES.join(", ")
        )));
    }
    let id = writable_project(app, "roadmap")?;
    crate::project::set_key(&app.store, &id, ROADMAP_STATUS, status)?;
    self_record(app);
    Ok(exit::OK)
}

/// Print, replace or clear one of the plan singletons.
fn singleton(
    app: &App,
    which: Singleton,
    set_file: Option<&std::path::Path>,
    stdin: bool,
    clear: bool,
) -> Result<i32> {
    if !clear && set_file.is_none() && !stdin {
        return print_singleton(app, which.noun, which.path, which.status_key);
    }
    let id = writable_project(app, which.noun)?;
    let path = (which.path)(&app.store, &id);
    if clear {
        return clear_file(app, &path, which.noun);
    }
    // The baseline is what the file looked like before the caller's text
    // arrived, however long that took.
    let seen = crate::atomic::read_mtime(&path);
    let text = set_text(set_file)?;
    let first = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or_default();
    // Either header is accepted here: a roadmap filed as the current plan
    // is a mistake `workflow run` diagnoses on its own, not one mem refuses.
    if header_slug(first, "plan").is_none() && header_slug(first, "roadmap").is_none() {
        return Err(exit::usage(
            "a plan starts with `# plan: <slug>` or `# roadmap: <slug>`; to empty it use --clear",
        ));
    }
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    let (text, kept) = crate::write::carry_ticks(&current, &text);
    if !kept.is_empty() {
        eprintln!(
            "mem: kept the tick on {}: the {} has them ticked and the incoming copy did not",
            kept.join(", "),
            which.noun
        );
    }
    land(app, &path, &text, seen, which.file)
}

/// `mem plan <slug>` — one milestone's plan, filed under `plans/<slug>.md`.
fn stored_plan(
    app: &App,
    slug: &str,
    set_file: Option<&std::path::Path>,
    stdin: bool,
    clear: bool,
) -> Result<i32> {
    if !clear && set_file.is_none() && !stdin {
        return print_slug_file(
            app,
            slug,
            |store, id, slug| store.plan_slot(id, slug),
            format!("no stored plan '{slug}' — `mem plan --list` lists them"),
        );
    }
    let id = writable_project(app, "plan")?;
    let path = app.store.plan_slot(&id, slug);
    if clear {
        return clear_file(app, &path, "stored plan");
    }
    let seen = crate::atomic::read_mtime(&path);
    let text = set_text(set_file)?;
    // The header is the file name. A plan filed under a slug that is not its
    // own is what would later send a run at the wrong milestone.
    let first = text.lines().next().unwrap_or_default();
    if header_slug(first, "plan") != Some(slug) && !is_plan_page(&text) {
        return Err(exit::usage(format!(
            "a stored plan's first line must be `# plan: {slug}`, and this one is `{}`",
            crate::search::truncate_bytes(first.trim(), 60)
        )));
    }
    land(app, &path, &text, seen, &format!("{slug}.md"))
}

/// A plan page: a whole HTML document with a designed page's `data-plan`
/// root. The slug it is filed under is its only name, since the page has no
/// header line.
fn is_plan_page(text: &str) -> bool {
    let first = text.trim_start().get(..15).unwrap_or_default();
    first.eq_ignore_ascii_case("<!doctype html>") && text.contains("data-plan=")
}

/// The slug of a `# <noun>: <slug>` header line, read as the plan parser
/// reads it: one hash, the word, and the slug alone to the end of the line.
fn header_slug<'a>(line: &'a str, noun: &str) -> Option<&'a str> {
    let rest = line.trim_start().strip_prefix('#')?.trim_start();
    let slug = rest.strip_prefix(noun)?.strip_prefix(':')?.trim();
    (!slug.is_empty()).then_some(slug)
}

/// The project a plan write belongs to, or the usage error that says so.
fn writable_project(app: &App, noun: &str) -> Result<String> {
    let identity = app.identity(Mode::Write)?;
    identity.id().map(str::to_string).ok_or_else(|| {
        exit::usage(format!(
            "a {noun} belongs to a project — run this in a checkout or pass --project"
        ))
    })
}

/// Clearing what is not there is not an error: `--clear` states the end it
/// wants, not a transition it expects to make.
fn clear_file(app: &App, path: &std::path::Path, noun: &str) -> Result<i32> {
    match std::fs::remove_file(path) {
        Ok(()) => {
            self_record(app);
            Ok(exit::OK)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(exit::OK),
        Err(e) => Err(exit::store_error(format!("clearing the {noun}: {e}"))),
    }
}

fn set_text(set_file: Option<&std::path::Path>) -> Result<String> {
    match set_file {
        Some(file) => std::fs::read_to_string(file)
            .map_err(|e| exit::not_found(format!("{}: {e}", file.display()))),
        None => crate::write::read_stdin(),
    }
}

/// The CAS write every plan file lands through, named so the conflict says
/// which file changed under the caller.
fn land(
    app: &App,
    path: &std::path::Path,
    text: &str,
    seen: Option<std::time::SystemTime>,
    file: &str,
) -> Result<i32> {
    match crate::write::write_singleton_since(path, text, seen)? {
        crate::write::SingletonWrite::Conflict => Err(exit::coded(
            exit::CAS_CONFLICT,
            format!("{file} changed since it was read — re-read it and try again"),
        )),
        _ => {
            self_record(app);
            Ok(exit::OK)
        }
    }
}

/// `mem plan --tick <task-id>` and `mem roadmap --tick <slug>`, the CAS-safe
/// checkbox write. The project is resolved in read mode: a tick can only
/// apply to a file that already exists, so there is never a project to
/// invent here.
fn tick_singleton(app: &App, which: Singleton, task: &str) -> Result<i32> {
    let identity = app.identity(Mode::Read)?;
    let Some(id) = identity.id() else {
        return Err(exit::not_found(format!(
            "no {} here — this is not a mem project",
            which.noun
        )));
    };
    let path = (which.path)(&app.store, id);
    let outcome = crate::write::tick_task(&path, task, which.item)?;
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
pub fn wiki(
    app: &App,
    slug: Option<&str>,
    stdin: bool,
    sections: bool,
    note: Option<&str>,
    rebuild: bool,
) -> Result<i32> {
    // `<slug>#<hslug>` addresses one section; the slug alone, the whole page.
    let (slug, hslug) = match slug.map(|s| s.split_once('#').unwrap_or((s, ""))) {
        Some((slug, "")) => (Some(slug), None),
        Some((slug, hslug)) => (Some(slug), Some(hslug)),
        None => (None, None),
    };
    if let Some(slug) = slug {
        check_slug(slug, "page")?;
    }
    if slug == Some(crate::lint::LINT) {
        if hslug.is_some() || stdin || note.is_some() || sections || rebuild {
            return Err(exit::usage(
                "lint is a reserved slug — `mem wiki lint` checks the wiki, so no page \
                 may be called that",
            ));
        }
        return crate::lint::lint(app);
    }
    if rebuild {
        if slug != Some(WIKI_INDEX) || hslug.is_some() {
            return Err(exit::usage(
                "--rebuild rewrites the index page — `mem wiki index --rebuild`",
            ));
        }
        return crate::lint::rebuild_index(app);
    }
    if stdin || note.is_some() {
        let Some(slug) = slug else {
            return Err(exit::usage(
                "name the page to write, e.g. `mem wiki index --stdin --note \"why\"`",
            ));
        };
        return wiki_write(app, slug, hslug, stdin, note);
    }
    if sections && (slug.is_none() || hslug.is_some()) {
        return Err(exit::usage(
            "--sections lists one page's sections, e.g. `mem wiki index --sections`",
        ));
    }
    match (slug, hslug) {
        (Some(slug), Some(hslug)) => wiki_print_section(app, slug, hslug),
        (Some(slug), None) if sections => wiki_sections(app, slug),
        (Some(slug), None) => wiki_print(app, slug),
        (None, _) => wiki_list(app),
    }
}

fn wiki_list(app: &App) -> Result<i32> {
    list_slug_files(
        app,
        |store, id| store.wiki_pages(id),
        "pages",
        "no pages yet — write one with `mem wiki <slug> --stdin --note \"why\"`",
    )
}

/// A page prints byte for byte, like plan.md: hub renders it and
/// a session reads it, and neither wants mem's opinion about markdown.
fn wiki_print(app: &App, slug: &str) -> Result<i32> {
    print_slug_file(
        app,
        slug,
        |store, id, slug| store.wiki_page(id, slug),
        format!("no page '{slug}' — `mem wiki` lists them"),
    )
}

/// The page a section verb reads, or None once the missing page is reported.
fn read_page(app: &App, slug: &str) -> Result<Option<(std::path::PathBuf, String)>> {
    let identity = app.identity(Mode::Read)?;
    let found = identity
        .id()
        .map(|id| app.store.wiki_page(id, slug))
        .and_then(|path| std::fs::read(&path).ok().map(|bytes| (path, bytes)));
    let Some((path, bytes)) = found else {
        if !app.quiet && !app.json {
            eprintln!(
                "mem: {}",
                unknown_project_note(&identity)
                    .unwrap_or_else(|| format!("no page '{slug}' — `mem wiki` lists them"))
            );
        }
        return Ok(None);
    };
    Ok(Some((path, String::from_utf8_lossy(&bytes).into_owned())))
}

/// The refusal for a section the page lacks names the ones it has, so the
/// next try needs no listing first.
fn no_such_section(slug: &str, hslug: &str, sections: &[crate::sections::PageSection]) -> String {
    let have: Vec<&str> = sections.iter().map(|s| s.hslug.as_str()).collect();
    format!(
        "no section '{hslug}' in {slug} — its sections: {}",
        if have.is_empty() {
            "none".to_string()
        } else {
            have.join(", ")
        }
    )
}

fn wiki_sections(app: &App, slug: &str) -> Result<i32> {
    let Some((_, text)) = read_page(app, slug)? else {
        return Ok(exit::NOT_FOUND);
    };
    let sections = crate::sections::split(&text);
    if app.json {
        let rows: Vec<serde_json::Value> = sections
            .iter()
            .map(|s| json!({ "hslug": s.hslug, "heading": s.heading, "bytes": s.end - s.start }))
            .collect();
        println!("{}", serde_json::to_string(&json!({ "sections": rows }))?);
    } else {
        for s in &sections {
            println!("{}  {}  {}", s.hslug, s.end - s.start, s.heading);
        }
    }
    Ok(exit::OK)
}

/// One section prints byte for byte, heading line included, as the page does.
fn wiki_print_section(app: &App, slug: &str, hslug: &str) -> Result<i32> {
    let Some((path, text)) = read_page(app, slug)? else {
        return Ok(exit::NOT_FOUND);
    };
    let sections = crate::sections::split(&text);
    let Some(section) = sections.iter().find(|s| s.hslug == hslug) else {
        if !app.quiet && !app.json {
            eprintln!("mem: {}", no_such_section(slug, hslug, &sections));
        }
        return Ok(exit::NOT_FOUND);
    };
    let body = &text[section.start..section.end];
    if app.json {
        println!(
            "{}",
            serde_json::to_string(&json!({
                "slug": slug,
                "hslug": section.hslug,
                "heading": section.heading,
                "text": body,
                "bytes": body.len(),
                "path": path.to_string_lossy(),
            }))?
        );
    } else {
        print!("{body}");
    }
    Ok(exit::OK)
}

/// A slug is a file name, so this is what keeps `..`, dot-temps and bisync
/// conflict losers out of the directories a slug addresses.
fn check_slug(slug: &str, what: &str) -> Result<()> {
    if crate::store::is_valid_slug(slug) {
        return Ok(());
    }
    Err(exit::usage(format!(
        "'{slug}' is not a {what} slug — lower case letters, digits and dashes, \
         starting with a letter or a digit, at most {} characters",
        crate::store::SLUG_MAX
    )))
}

/// The listing both slug-addressed readers print: one line per file, and the
/// same row in JSON under the caller's key.
fn list_slug_files(
    app: &App,
    which: fn(&crate::store::Store, &str) -> Vec<crate::store::Page>,
    key: &str,
    empty: &str,
) -> Result<i32> {
    let identity = app.identity(Mode::Read)?;
    let files = match identity.id() {
        Some(id) => which(&app.store, id),
        None => Vec::new(),
    };
    if app.json {
        let rows: Vec<serde_json::Value> = files
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
        println!("{}", serde_json::to_string(&json!({ key: rows }))?);
    } else {
        for p in &files {
            println!(
                "{:<24} {:>6}  {}  {}",
                p.slug,
                p.bytes,
                date(p.modified_epoch),
                p.title
            );
        }
    }
    if files.is_empty() {
        if !app.quiet && !app.json {
            eprintln!(
                "mem: {}",
                unknown_project_note(&identity).unwrap_or_else(|| empty.to_string())
            );
        }
        return Ok(exit::NOT_FOUND);
    }
    Ok(exit::OK)
}

/// One slug-addressed file, byte for byte, with the same JSON around it.
fn print_slug_file(
    app: &App,
    slug: &str,
    which: fn(&crate::store::Store, &str, &str) -> std::path::PathBuf,
    missing: String,
) -> Result<i32> {
    let identity = app.identity(Mode::Read)?;
    let found = identity
        .id()
        .map(|id| which(&app.store, id, slug))
        .and_then(|path| std::fs::read(&path).ok().map(|bytes| (path, bytes)));
    let Some((path, bytes)) = found else {
        if !app.quiet && !app.json {
            eprintln!(
                "mem: {}",
                unknown_project_note(&identity).unwrap_or(missing)
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

fn wiki_write(
    app: &App,
    slug: &str,
    hslug: Option<&str>,
    stdin: bool,
    note: Option<&str>,
) -> Result<i32> {
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
    // The CAS baseline is what the page looked like before the writer's text
    // arrived, and `--stdin` holds that window open for as long as the writer
    // takes.
    let seen = crate::atomic::read_mtime(&path);
    // A section write splices into the page as it was when the baseline was
    // taken, so the section must exist before stdin is read.
    let target = match hslug {
        Some(hslug) => {
            let Ok(bytes) = std::fs::read(&path) else {
                return Err(exit::not_found(format!(
                    "no page '{slug}' — `mem wiki` lists them"
                )));
            };
            let page = String::from_utf8_lossy(&bytes).into_owned();
            let sections = crate::sections::split(&page);
            let Some(section) = sections.iter().find(|s| s.hslug == hslug).cloned() else {
                return Err(exit::not_found(no_such_section(slug, hslug, &sections)));
            };
            Some((page, section))
        }
        None => None,
    };
    let mut text = crate::write::read_stdin()?;
    if text.trim().is_empty() {
        return Err(exit::usage(
            "a page needs text — there is no delete verb, so a page that is done \
             becomes a one-line stub pointing at what replaced it",
        ));
    }
    if let Some((page, section)) = &target {
        // Only the preamble has no heading of its own: text without one would
        // fold this section into the one above it.
        if section.hslug != crate::sections::TOP && !text.starts_with("## ") {
            return Err(exit::usage(format!(
                "the text for {slug}#{} must open on its `## ` heading line",
                section.hslug
            )));
        }
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text = format!("{}{text}{}", &page[..section.start], &page[section.end..]);
    }
    let label = match hslug {
        Some(hslug) => format!("{slug}#{hslug}"),
        None => slug.to_string(),
    };
    if let crate::write::SingletonWrite::Conflict =
        crate::write::write_singleton_since(&path, &text, seen)?
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
        &format!("wiki {label}: {note}"),
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
        println!("wiki {label}  #{}", written.short_id);
    }
    Ok(exit::OK)
}

/// plan.md and roadmap.md are printed byte for byte: the hub and the skills
/// parse them, so this is a contract, not a display.
fn print_singleton(
    app: &App,
    noun: &str,
    which: fn(&crate::store::Store, &str) -> std::path::PathBuf,
    status_key: Option<&str>,
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
                let mut out = json!({
                    "text": String::from_utf8_lossy(&bytes),
                    "path": path.to_string_lossy(),
                });
                if let Some(status) =
                    status_key.and_then(|key| crate::project::declared(&app.store, id, key))
                {
                    out["status"] = json!(status);
                }
                println!("{}", serde_json::to_string(&out)?);
            } else {
                print!("{}", String::from_utf8_lossy(&bytes));
            }
            Ok(exit::OK)
        }
        Err(_) => {
            if !app.quiet && !app.json {
                eprintln!("mem: no {noun} recorded for this project{}", remedy(noun));
            }
            Ok(exit::NOT_FOUND)
        }
    }
}

/// The next move when a singleton is missing, so an empty `mem plan` hands a
/// caller the verb that fills it instead of a dead end.
fn remedy(noun: &str) -> &'static str {
    match noun {
        "plan" => " — write one with `mem plan --stdin`",
        "roadmap" => " — write one with `mem roadmap --stdin`",
        _ => "",
    }
}

/// `mem ask` — writes the question, asks for a sync, fires a notification and
/// returns. It never waits: a tool call that blocks for hours dies at the
/// runtime's ceiling, so waiting is `mem questions --wait`.
///
/// A question is a person's, which is what the hub shows, unless `--for`
/// gives it to the orchestrator.
pub fn ask(
    app: &App,
    question: &str,
    options: &[String],
    recommend: Option<&str>,
    audience: Option<crate::cli::Audience>,
    about: Option<&str>,
) -> Result<i32> {
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
    meta.recommend = recommend.map(str::to_string);
    meta.about = about.map(str::to_string);
    let audience = audience.and_then(crate::cli::Audience::stored);
    meta.audience = audience.map(str::to_string);
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
                "recommend": recommend,
                "audience": audience,
            }))?
        );
    } else {
        println!("#{}", written.short_id);
    }
    Ok(exit::OK)
}

/// `mem questions` and `mem questions --wait <id>`.
///
/// `--for` narrows the listing to one audience's questions: the hub asks for
/// a person's, an orchestrator for its own. The JSON carries the body and the
/// answer, so a reader needs no second verb.
pub fn questions(
    app: &App,
    pending: bool,
    all_projects: bool,
    audience: Option<crate::cli::Audience>,
    wait: Option<&str>,
    timeout: &str,
) -> Result<i32> {
    // A wait resolves its own scope from the question, so this checkout's
    // identity — and the git call behind it — is only the listing's business.
    if let Some(id) = wait {
        return wait_for(app, id, timeout);
    }

    let index = app.read_index()?;
    let identity = app.identity(Mode::Read)?;
    let mut rows = if all_projects {
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
    if let Some(audience) = audience {
        rows.retain(|row| row.audience.as_deref() == audience.stored());
    }

    if app.json {
        // The text mode has its ✓/? column; the JSON carries the same fact,
        // or a robot reading the recent listing cannot tell answered from
        // pending (found live, the first day a human answered one).
        let mut items: Vec<serde_json::Value> = Vec::with_capacity(rows.len());
        for row in &rows {
            let mut v = row_json(row);
            let answer = index.answer_to(&row.id)?;
            v["answered"] = json!(answer.is_some());
            v["body"] = json!(read_body(row).trim());
            v["answer"] = json!(answer.as_ref().map(|a| read_body(a).trim().to_string()));
            // The index keeps neither field, so they come from the item file.
            let meta = read_item(row).map(|i| i.meta);
            let options = meta.as_ref().and_then(|m| m.options.clone());
            v["options"] = json!(options.unwrap_or_default());
            v["recommend"] = json!(meta.and_then(|m| m.recommend));
            items.push(v);
        }
        println!("{}", serde_json::to_string(&json!({ "questions": items }))?);
    } else {
        for row in &rows {
            let answered = index.answer_to(&row.id)?.is_some();
            // An orchestrator's question says so, so a reader can tell it
            // from one a person owes an answer to.
            let who = match &row.audience {
                Some(a) => format!("[{a}] "),
                None => String::new(),
            };
            println!(
                "{} #{}  {who}{}",
                if answered { "✓" } else { "?" },
                row.short_id,
                row.title
            );
            if let Some(text) = read_item(row).and_then(|i| i.meta.recommend) {
                println!("    recommended: {text}");
            }
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

/// `mem questions --about <prefix>` and `mem log --about <prefix>`: every
/// question, or every other item, about a part of a page, oldest first, the way a thread reads, each
/// with its text, and a question with its answer. `about` lives only in the
/// item's file, so every item of the kind is read.
pub fn about(app: &App, kind: &str, prefix: &str) -> Result<i32> {
    let identity = app.identity(Mode::Read)?;
    let index = app.read_index()?;
    let kind = (kind == "question").then_some(kind);
    let mut rows = index.recent_filtered(kind, None, identity.id(), usize::MAX >> 1)?;
    // A question is read with its answer through `questions --about`.
    rows.retain(|row| kind.is_some() || row.kind != "question");
    rows.retain(|row| {
        read_item(row)
            .and_then(|i| i.meta.about)
            .is_some_and(|about| about.starts_with(prefix))
    });
    rows.reverse();
    let mut items = Vec::with_capacity(rows.len());
    for row in &rows {
        let mut v = row_json(row);
        v["body"] = json!(read_body(row).trim());
        if kind.is_some() {
            let answer = index.answer_to(&row.id)?;
            v["answered"] = json!(answer.is_some());
            v["answer"] = json!(answer.as_ref().map(|a| read_body(a).trim().to_string()));
        }
        items.push(v);
    }
    let key = if kind.is_some() { "questions" } else { "items" };
    if app.json {
        println!("{}", serde_json::to_string(&json!({ key: items }))?);
    } else {
        for row in &rows {
            println!("#{}  {}", row.short_id, row.title);
        }
    }
    Ok(if rows.is_empty() {
        exit::NOT_FOUND
    } else {
        exit::OK
    })
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
pub(crate) const WIKI_INDEX: &str = "index";

/// Past this a page is a page to compact. There is no hard cap — a wiki grows
/// by being written to, and compaction is a verb, not a refusal.
const WIKI_PAGE_WARN_BYTES: u64 = 8 * 1024;

/// Up to this size, a page that links to a live page and is out of the index
/// reads as a finished page's stub rather than as index drift.
const WIKI_STUB_MAX_BYTES: usize = 512;

/// The slug a markdown link points at, when it points at a page in the same
/// wiki. `[name](name.md)` is the whole convention: an anchor is trimmed, and a
/// target with a scheme or a slash in it lives in some other tree.
pub(crate) fn page_link_target(target: &str) -> Option<&str> {
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
pub(crate) fn markdown_links(text: &str) -> Vec<&str> {
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
pub(crate) fn wiki_findings(
    app: &App,
    project: &crate::project::Project,
) -> Vec<crate::maint::Finding> {
    use crate::maint::{finding, looks_like_a_secret};
    let mut findings = Vec::new();
    let pages = app.store.wiki_pages(&project.id);
    if pages.is_empty() {
        return findings;
    }
    let slugs: std::collections::HashSet<&str> = pages.iter().map(|p| p.slug.as_str()).collect();
    let mut listed: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut stubs: std::collections::HashSet<String> = std::collections::HashSet::new();
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
        // A finished page becomes a one-line stub pointing at its replacement
        // and leaves the index (deletion comes back on the next sync). Short
        // plus a link to a live page reads as that stub; a stub whose link
        // dangles is still caught by the link check below.
        let mut points_at_a_page = false;
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
            } else {
                points_at_a_page = true;
            }
        }
        if !is_index && text.len() <= WIKI_STUB_MAX_BYTES && points_at_a_page {
            stubs.insert(page.slug.clone());
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
        if page.slug != WIKI_INDEX
            && !listed.contains(page.slug.as_str())
            && !stubs.contains(page.slug.as_str())
        {
            findings.push(finding(
                "wiki index",
                format!("{}/{}.md is not in the index", project.name, page.slug),
            ));
        }
    }
    findings
}

/// Puts the shipped pi extension back, making its directory first.
fn write_pi_extension(path: &std::path::Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::atomic::write_atomic(path, crate::maint::PI_EXTENSION.as_bytes())
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
    findings.extend(crate::maint::hook_findings(&app.dirs.claude_settings()));

    let pi_extension = app.dirs.pi_extension();
    if fix && !crate::maint::pi_extension_findings(&pi_extension).is_empty() {
        // A directory nobody may write is a finding like any other, not an end
        // to the fixes below it.
        findings.push(match write_pi_extension(&pi_extension) {
            Ok(()) => finding("pi extension", format!("wrote {}", pi_extension.display())),
            Err(e) => finding(
                "pi extension",
                format!("{} could not be written: {e}", pi_extension.display()),
            ),
        });
    } else {
        findings.extend(crate::maint::pi_extension_findings(&pi_extension));
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
        let check = if crate::store::is_conflict_name(&name) {
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
        findings.extend(wiki_findings(app, p));
    }

    // Child projects: a parent that is gone, a chain deeper than the one
    // level the contract allows, and a subdir vanished from a checkout this
    // machine has. A checkout absent here is no finding — the path map only
    // speaks for this machine.
    let path_map = crate::project::PathMap::load(&app.dirs.paths_toml());
    for p in &registry.projects {
        let Some(parent_id) = p.parent.as_deref() else {
            continue;
        };
        let Some(parent) = registry.by_id(parent_id) else {
            findings.push(finding(
                "child",
                format!(
                    "{}'s parent {parent_id} is no project the store knows",
                    p.name
                ),
            ));
            continue;
        };
        if parent.parent.is_some() {
            findings.push(finding(
                "child",
                format!("{} is a child of the child {}", p.name, parent.name),
            ));
        }
        let Some(subdir) = p.subdir.as_deref() else {
            findings.push(finding(
                "child",
                format!("{} names a parent but no subdir", p.name),
            ));
            continue;
        };
        for common in path_map.projects.get(parent_id).into_iter().flatten() {
            let common = std::path::Path::new(common);
            if common.file_name() == Some(std::ffi::OsStr::new(".git"))
                && let Some(top) = common.parent()
                && top.is_dir()
                && !top.join(subdir).is_dir()
            {
                findings.push(finding(
                    "child",
                    format!(
                        "{}'s subdir {subdir} is missing from {}",
                        p.name,
                        top.display()
                    ),
                ));
            }
        }
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
