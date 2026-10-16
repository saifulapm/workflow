//! Sharing a project's files with the other machines. A write to a project
//! whose project.toml says `share = "true"` goes to this machine's outbox
//! before it lands in the replica, so a mem killed between the two still has
//! the write to send. The local hub carries the outbox to the store and hands
//! the store's messages back to `mem share apply`, which lands other
//! machines' writes in the replica and turns a refused page into a conflict
//! file.
//!
//! A page write names the write its copy came from (`base`), and the store
//! refuses one whose base is no longer the page's head. An item is never
//! refused.

use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::{Value, json};
use ulid::Ulid;

/// What the write hook needs, set once per process by `App::new`.
struct Ctx {
    root: PathBuf,
    db: PathBuf,
    socket: PathBuf,
}

static CTX: OnceLock<Ctx> = OnceLock::new();

pub fn install(root: PathBuf, db: PathBuf, socket: PathBuf) {
    let _ = CTX.set(Ctx { root, db, socket });
}

/// Called by every store write before the file is replaced. Returns whether
/// the write went to the outbox, so the caller kicks the hub once the replica
/// has it too.
pub fn before_write(path: &Path, body: Option<&[u8]>) -> Result<bool> {
    let Some(ctx) = CTX.get() else {
        return Ok(false);
    };
    Ok(queue(&ctx.db, &ctx.root, path, body)?.is_some())
}

/// Tells the local hub there is something to send. Best effort: the outbox
/// keeps the write, and the hub reads it whenever it next connects.
pub fn kick() {
    if let Some(ctx) = CTX.get() {
        crate::localhub::kick(&ctx.socket);
    }
}

/// Puts the write of `path` in the outbox when its project shares, and
/// returns the write's id; None when the path is not shared.
pub fn queue(db: &Path, root: &Path, path: &Path, body: Option<&[u8]>) -> Result<Option<String>> {
    let Some((rel, false)) = shared(root, path, is_switched(db)) else {
        return Ok(None);
    };
    let body = match body.map(std::str::from_utf8) {
        Some(Ok(text)) => Some(text),
        None => None,
        // Only text is shared this way; bytes travel on their own.
        Some(Err(_)) => return Ok(None),
    };
    let mut conn = open(db)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let base: Option<String> = if is_item(&rel) {
        None
    } else {
        head(&tx, &rel)?
    };
    let id = Ulid::generate().to_string();
    tx.execute(
        "INSERT INTO outbox(id, path, base, body) VALUES (?1, ?2, ?3, ?4)",
        params![id, rel, base, body],
    )?;
    set_head(&tx, &rel, &id)?;
    tx.commit()?;
    Ok(Some(id))
}

/// Puts an evidence or raw file of a shared project on the list the hub
/// uploads to the project's files store, and kicks the hub.
pub fn queue_file(path: &Path) -> Result<()> {
    let Some(ctx) = CTX.get() else {
        return Ok(());
    };
    let Some((rel, true)) = shared(&ctx.root, path, is_switched(&ctx.db)) else {
        return Ok(());
    };
    open(&ctx.db)?.execute("INSERT OR IGNORE INTO files(path) VALUES (?1)", [rel])?;
    kick();
    Ok(())
}

/// Fetches an evidence or raw file of a shared project that this replica
/// has the record of but not the bytes, through the local hub. False when
/// there is nothing to fetch or the hub could not get it.
pub fn fetch_missing(path: &Path) -> bool {
    let Some(ctx) = CTX.get() else {
        return false;
    };
    let Some((rel, true)) = shared(&ctx.root, path, is_switched(&ctx.db)) else {
        return false;
    };
    if path.exists() {
        return false;
    }
    crate::localhub::file(&ctx.socket, &rel)
        .is_some_and(|bytes| crate::atomic::write_replica(path, &bytes).is_ok())
}

/// `path` relative to the store when it belongs to a shared project, with
/// whether it is evidence or raw bytes (shared as a file) rather than text
/// (shared as a write). On a switched machine every project shares, and so
/// do the global items. Temps and conflict copies are never shared.
fn shared(root: &Path, path: &Path, switched: bool) -> Option<(String, bool)> {
    let rel = path.strip_prefix(root).ok()?;
    let parts: Vec<&str> = rel.iter().map(|p| p.to_str()).collect::<Option<_>>()?;
    let [first, second, rest @ ..] = parts.as_slice() else {
        return None;
    };
    let name = rest.last()?;
    if name.starts_with('.') || crate::store::is_conflict_name(name) {
        return None;
    }
    let rel = parts.join("/");
    match (*first, *second, rest) {
        ("global", "items", [_]) => switched.then_some((rel, false)),
        ("projects", project, _) => {
            let store = crate::store::Store::new(root.to_path_buf());
            let bytes = matches!(rest.first(), Some(&"evidence" | &"raw"));
            (switched
                || crate::project::declared(&store, project, "share").as_deref() == Some("true"))
            .then_some((rel, bytes))
        }
        _ => None,
    }
}

/// Whether this machine has switched to the store: a flag beside the
/// sharing database, never synced.
pub fn is_switched(db: &Path) -> bool {
    db.with_file_name("switched").exists()
}

/// Switches this machine to the store: from now on every project and the
/// global items share, and the next hello asks for every write again, so
/// the replica learns the head of each path the store already holds.
pub fn switch(db: &Path) -> Result<()> {
    open(db)?;
    let flag = db.with_file_name("switched");
    std::fs::write(&flag, "").with_context(|| format!("writing {}", flag.display()))?;
    resync(db)
}

fn is_item(rel: &str) -> bool {
    rel.contains("/items/")
}

fn open(db: &Path) -> Result<Connection> {
    if let Some(dir) = db.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let conn = Connection::open(db).with_context(|| format!("opening {}", db.display()))?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = FULL;
         CREATE TABLE IF NOT EXISTS outbox(
             n INTEGER PRIMARY KEY AUTOINCREMENT,
             id TEXT NOT NULL UNIQUE,
             path TEXT NOT NULL,
             base TEXT,
             body TEXT);
         CREATE TABLE IF NOT EXISTS heads(path TEXT PRIMARY KEY, id TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS skipped(path TEXT PRIMARY KEY, id TEXT NOT NULL, seq INTEGER NOT NULL, body TEXT);
         CREATE TABLE IF NOT EXISTS files(n INTEGER PRIMARY KEY AUTOINCREMENT, path TEXT NOT NULL UNIQUE);
         CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value INTEGER NOT NULL);
         CREATE TABLE IF NOT EXISTS imported(id TEXT PRIMARY KEY);
         CREATE TABLE IF NOT EXISTS seeded_files(path TEXT PRIMARY KEY);",
    )?;
    Ok(conn)
}

fn head(conn: &Connection, rel: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT id FROM heads WHERE path = ?1", [rel], |r| r.get(0))
        .optional()?)
}

fn set_head(conn: &Connection, rel: &str, id: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO heads(path, id) VALUES (?1, ?2) \
         ON CONFLICT(path) DO UPDATE SET id = excluded.id",
        params![rel, id],
    )?;
    Ok(())
}

fn seq(conn: &Connection) -> Result<u64> {
    Ok(conn
        .query_row("SELECT value FROM meta WHERE key = 'seq'", [], |r| {
            r.get::<_, i64>(0)
        })
        .optional()?
        .unwrap_or(0) as u64)
}

fn raise_seq(conn: &Connection, seen: u64) -> Result<()> {
    conn.execute(
        "INSERT INTO meta(key, value) VALUES ('seq', ?1) \
         ON CONFLICT(key) DO UPDATE SET value = max(value, excluded.value)",
        [seen as i64],
    )?;
    Ok(())
}

fn waiting_for(conn: &Connection, rel: &str) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM outbox WHERE path = ?1 LIMIT 1",
            [rel],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// The hello the hub sends first on every connection.
pub fn hello(db: &Path, machine: &str) -> Result<Value> {
    let conn = open(db)?;
    // A fresh connection's hello is itself the replay a resync asks for.
    conn.execute("DELETE FROM meta WHERE key = 'resync'", [])?;
    Ok(json!({"t": "hello", "machine": machine, "since": seq(&conn)?}))
}

/// The outbox, oldest first, as the store's `write` messages, then each file
/// still to upload as `{"t":"file","path","local"}`, `local` being where the
/// bytes are on this machine.
pub fn pending(db: &Path, root: &Path, machine: &str) -> Result<Vec<Value>> {
    let conn = open(db)?;
    let resync = conn
        .query_row("SELECT 1 FROM meta WHERE key = 'resync'", [], |_| Ok(()))
        .optional()?
        .is_some();
    let mut first = Vec::new();
    if resync {
        first.push(json!({"t": "hello", "machine": machine, "since": seq(&conn)?}));
    }
    let mut stmt = conn.prepare("SELECT id, path, base, body FROM outbox ORDER BY n")?;
    let rows = stmt.query_map([], |r| {
        Ok(json!({
            "t": "write",
            "id": r.get::<_, String>(0)?,
            "path": r.get::<_, String>(1)?,
            "base": r.get::<_, Option<String>>(2)?,
            "body": r.get::<_, Option<String>>(3)?,
        }))
    })?;
    let mut out = first;
    out.extend(rows.collect::<rusqlite::Result<Vec<Value>>>()?);
    let mut stmt = conn.prepare("SELECT path FROM files ORDER BY n")?;
    for rel in stmt.query_map([], |r| r.get::<_, String>(0))? {
        let rel = rel?;
        let local = root.join(&rel);
        out.push(json!({"t": "file", "path": rel, "local": local.to_string_lossy()}));
    }
    Ok(out)
}

/// What `doctor` says about sharing: the last sequence seen and how many
/// writes wait. None on a machine that never shared.
pub fn status(db: &Path) -> Result<Option<(u64, u64)>> {
    if !db.exists() {
        return Ok(None);
    }
    let conn = open(db)?;
    let waiting: i64 = conn.query_row("SELECT count(*) FROM outbox", [], |r| r.get(0))?;
    Ok(Some((seq(&conn)?, waiting as u64)))
}

/// Applies one message from the store. A message mem cannot use is skipped:
/// the store's reply to something else must not stop the ones behind it.
/// With `all`, a record lands whether or not its project shares here.
pub fn apply(db: &Path, root: &Path, message: &Value, all: bool) -> Result<()> {
    let mut conn = open(db)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let text = |v: &Value| v.as_str().map(str::to_string);
    match message["t"].as_str() {
        Some("rec") => {
            let (Some(id), Some(rel), Some(seen)) = (
                text(&message["id"]),
                text(&message["path"]),
                message["seq"].as_u64(),
            ) else {
                return Ok(());
            };
            let Some(file) = replica_path(root, &rel) else {
                return Ok(());
            };
            // The store carries every machine's shared projects; this
            // replica lands only the ones it shares too.
            if !all && shared(root, &file, is_switched(db)).is_none() {
                raise_seq(&tx, seen)?;
                tx.commit()?;
                return Ok(());
            }
            let body = message["body"].as_str();
            if head(&tx, &rel)?.as_deref() == Some(id.as_str()) {
                // This replica already holds it: its own write, or a replay.
            } else if waiting_for(&tx, &rel)? {
                // A local write still waiting wins for now. The store answers
                // it with a refusal carrying the newest version, or with an
                // ack, after which this record lands if it is newer.
                tx.execute(
                    "INSERT INTO skipped(path, id, seq, body) VALUES (?1, ?2, ?3, ?4) \
                     ON CONFLICT(path) DO UPDATE SET id = excluded.id, seq = excluded.seq, \
                     body = excluded.body WHERE excluded.seq > skipped.seq",
                    params![rel, id, seen as i64, body],
                )?;
            } else if file.is_dir() {
                // No file can ever stand there; passing over it keeps one bad
                // record from holding back every record behind it.
                eprintln!("mem: share apply: {rel} is a directory here; passing over it");
            } else {
                land(&file, body)?;
                set_head(&tx, &rel, &id)?;
            }
            raise_seq(&tx, seen)?;
        }
        Some("ack") => {
            let Some(id) = text(&message["id"]) else {
                return Ok(());
            };
            if let Some((rel, body)) = take(&tx, &id)?
                && !waiting_for(&tx, &rel)?
                && let Some(file) = replica_path(root, &rel)
            {
                let acked = message["seq"].as_i64().unwrap_or(i64::MAX);
                match take_skipped(&tx, &rel)? {
                    // Another machine wrote this path after mine while mine
                    // waited (a lost ack, then a replay): its version is the
                    // newer one. At the same seq it is this write itself,
                    // replayed to a replica that imported it and holds no
                    // head for it.
                    Some((theirs, seq, theirs_body)) if seq >= acked => {
                        land(&file, theirs_body.as_deref())?;
                        set_head(&tx, &rel, &theirs)?;
                    }
                    // An imported write the store appended just now, past
                    // every record this replica has seen, is the newest. A
                    // re-ack at or below that seq is a write this replica
                    // already passed, so nothing lands.
                    _ if acked > seq(&tx)? as i64 && was_imported(&tx, &id)? => {
                        land(&file, body.as_deref())?;
                        set_head(&tx, &rel, &id)?;
                    }
                    // The replica missed this write when mem died between the
                    // outbox and the rename; the ack is the moment to heal it.
                    _ if head(&tx, &rel)?.as_deref() == Some(id.as_str())
                        && std::fs::read_to_string(&file).ok() != body =>
                    {
                        land(&file, body.as_deref())?;
                    }
                    _ => {}
                }
            }
            if let Some(seen) = message["seq"].as_u64() {
                raise_seq(&tx, seen)?;
            }
        }
        Some("refused") => {
            let Some(id) = text(&message["id"]) else {
                return Ok(());
            };
            let head_msg = &message["head"];
            let Some((rel, body)) = take(&tx, &id)? else {
                return Ok(());
            };
            let Some(file) = replica_path(root, &rel) else {
                return Ok(());
            };
            if let Some(mine) = body {
                let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("page");
                let copy = file.with_file_name(format!("{name}.conflict-{}", id.to_lowercase()));
                crate::atomic::write_replica(&copy, mine.as_bytes())?;
            }
            if !waiting_for(&tx, &rel)?
                && let Some(theirs) = text(&head_msg["id"])
            {
                take_skipped(&tx, &rel)?;
                land(&file, head_msg["body"].as_str())?;
                set_head(&tx, &rel, &theirs)?;
            }
        }
        Some("stored") => {
            if let Some(rel) = text(&message["path"]) {
                tx.execute("DELETE FROM files WHERE path = ?1", [rel])?;
            }
        }
        Some("ready") => {
            if let Some(seen) = message["seq"].as_u64() {
                raise_seq(&tx, seen)?;
            }
            tx.execute("DELETE FROM meta WHERE key = 'resync'", [])?;
        }
        _ => {}
    }
    tx.commit()?;
    Ok(())
}

fn was_imported(conn: &Connection, id: &str) -> Result<bool> {
    Ok(conn
        .query_row("SELECT 1 FROM imported WHERE id = ?1", [id], |_| Ok(()))
        .optional()?
        .is_some())
}

/// Removes the newest record from elsewhere that was passed over for `rel`
/// while a local write waited, returning its id, seq and body.
fn take_skipped(conn: &Connection, rel: &str) -> Result<Option<(String, i64, Option<String>)>> {
    let row = conn
        .query_row(
            "SELECT id, seq, body FROM skipped WHERE path = ?1",
            [rel],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    conn.execute("DELETE FROM skipped WHERE path = ?1", [rel])?;
    Ok(row)
}

/// A project began sharing here after the store already held its writes,
/// which this replica counted past: ask for every write again. The hub sends
/// the hello `pending` prints, and the `ready` that ends the replay clears it.
pub fn resync(db: &Path) -> Result<()> {
    let conn = open(db)?;
    // A replica that has counted nothing yet passed nothing over.
    if seq(&conn)? == 0 {
        return Ok(());
    }
    conn.execute(
        "INSERT INTO meta(key, value) VALUES ('seq', 0) \
         ON CONFLICT(key) DO UPDATE SET value = 0",
        [],
    )?;
    conn.execute(
        "INSERT INTO meta(key, value) VALUES ('resync', 1) \
         ON CONFLICT(key) DO UPDATE SET value = 1",
        [],
    )?;
    Ok(())
}

/// Removes the outbox row for `id`, returning its path and body.
fn take(conn: &Connection, id: &str) -> Result<Option<(String, Option<String>)>> {
    let row = conn
        .query_row("SELECT path, body FROM outbox WHERE id = ?1", [id], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()?;
    conn.execute("DELETE FROM outbox WHERE id = ?1", [id])?;
    Ok(row)
}

/// Where a store path lives in this replica, or None for a path that would
/// leave the projects directory or the global items.
fn replica_path(root: &Path, rel: &str) -> Option<PathBuf> {
    let rel = Path::new(rel);
    if !rel.components().all(|c| matches!(c, Component::Normal(_))) {
        return None;
    }
    let parts: Vec<&str> = rel.iter().map(|p| p.to_str()).collect::<Option<_>>()?;
    let inside = matches!(
        parts.as_slice(),
        ["global", "items", _] | ["projects", _, _, ..]
    );
    inside.then(|| root.join(rel))
}

/// Writes or removes the replica's copy, never through the outbox: this is
/// another machine's write arriving.
fn land(file: &Path, body: Option<&str>) -> Result<()> {
    match body {
        Some(body) => crate::atomic::write_replica(file, body.as_bytes()),
        None => match std::fs::remove_file(file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(e).with_context(|| format!("removing {}", file.display()))
            }
            _ => Ok(()),
        },
    }
}

/// How long an evidence or raw file stays in the store; an older one lives
/// in the weekly snapshot alone.
pub const FILE_DAYS: u64 = 90;

/// The import's plan against the store's replay (its `rec` messages, in any
/// order): an import write for every text file here that the store lacks or
/// holds otherwise, based on the store's head of the path; a removal for each
/// path the store holds in a project here that this store no longer holds;
/// then each evidence or raw file younger than `FILE_DAYS` that no import
/// has uploaded yet. Text first, so the files come once the text is in.
///
/// Only a path whose head `machine` (this one) wrote is replaced or removed:
/// until it switches, everything under its name came from its own imports,
/// while a head another machine wrote may be that machine's work since its
/// switch, which a store kept by the bisync must not undo. Such a page that
/// differs here is kept beside it as `<name>.conflict-import`, which `mem
/// doctor` lists, and the plan says so with a `kept` line. Neither the
/// outbox nor the heads change: this machine
/// shares nothing yet.
pub fn seed(
    db: &Path,
    root: &Path,
    machine: &str,
    replay: impl IntoIterator<Item = Value>,
) -> Result<Vec<Value>> {
    type Head = (u64, String, Option<String>, String);
    let mut newest: std::collections::HashMap<String, Head> = std::collections::HashMap::new();
    for message in replay {
        let (Some("rec"), Some(path), Some(id), Some(seq)) = (
            message["t"].as_str(),
            message["path"].as_str(),
            message["id"].as_str(),
            message["seq"].as_u64(),
        ) else {
            continue;
        };
        if newest.get(path).is_none_or(|(kept, ..)| *kept < seq) {
            let body = message["body"].as_str().map(str::to_string);
            let by = message["machine"].as_str().unwrap_or_default().to_string();
            newest.insert(path.to_string(), (seq, id.to_string(), body, by));
        }
    }
    let write = |path: &str, base: Option<&str>, body: Option<&str>| {
        json!({"t": "write", "id": Ulid::generate().to_string(), "path": path,
               "base": base, "body": body, "import": true})
    };
    let mut texts = Vec::new();
    collect(root, &root.join("global/items"), false, &mut texts)?;
    let mut projects = Vec::new();
    collect(root, &root.join("projects"), true, &mut projects)?;
    let (bytes, text): (Vec<String>, Vec<String>) =
        projects.into_iter().partition(|rel| is_file_rel(rel));
    texts.extend(text);
    let mut plan = Vec::new();
    let mut here = std::collections::HashSet::new();
    for rel in &texts {
        here.insert(rel.as_str());
        let Ok(body) = std::fs::read_to_string(root.join(rel)) else {
            eprintln!("mem: share seed: {rel} is not text; passing over it");
            continue;
        };
        match newest.get(rel) {
            Some((_, _, Some(theirs), _)) if *theirs == body => {}
            Some((_, head, _, by)) if by == machine => {
                plan.push(write(rel, Some(head), Some(&body)))
            }
            Some(_) if is_item(rel) => {}
            Some(_) => {
                let copy = root.join(format!("{rel}.conflict-import"));
                crate::atomic::write_replica(&copy, body.as_bytes())?;
                plan.push(json!({"t": "kept", "path": rel}));
            }
            None => plan.push(write(rel, None, Some(&body))),
        }
    }
    let mut gone: Vec<(&String, &String)> = newest
        .iter()
        .filter(|(rel, (_, _, body, by))| {
            body.is_some()
                && by == machine
                && !here.contains(rel.as_str())
                && !is_file_rel(rel)
                && project_dir(root, rel).is_some_and(|dir| dir.is_dir())
        })
        .map(|(rel, (_, head, ..))| (rel, head))
        .collect();
    gone.sort();
    for (rel, head) in gone {
        plan.push(write(rel, Some(head), None));
    }
    let conn = open(db)?;
    let young = std::time::SystemTime::now() - std::time::Duration::from_secs(FILE_DAYS * 86_400);
    let mut files = Vec::new();
    for rel in bytes {
        let file = root.join(&rel);
        let fresh = std::fs::metadata(&file)
            .and_then(|m| m.modified())
            .is_ok_and(|at| at > young);
        let uploaded = conn
            .query_row("SELECT 1 FROM seeded_files WHERE path = ?1", [&rel], |_| {
                Ok(())
            })
            .optional()?
            .is_some();
        if fresh && !uploaded {
            files.push(json!({"t": "file", "path": rel, "local": file.to_string_lossy()}));
        }
    }
    plan.extend(files);
    Ok(plan)
}

/// Notes files the import uploaded, so the next run passes over them.
pub fn seeded(db: &Path, paths: impl IntoIterator<Item = String>) -> Result<()> {
    let conn = open(db)?;
    for path in paths {
        conn.execute(
            "INSERT OR IGNORE INTO seeded_files(path) VALUES (?1)",
            [path],
        )?;
    }
    Ok(())
}

/// Every file under `dir`, as a path relative to `root`, sorted; with
/// `deep`, in its subdirectories too. Dot names and conflict copies are not
/// mem's and are left out.
fn collect(root: &Path, dir: &Path, deep: bool, out: &mut Vec<String>) -> Result<()> {
    let mut entries: Vec<_> = match std::fs::read_dir(dir) {
        Ok(entries) => entries.collect::<std::io::Result<_>>()?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
    };
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with('.') || crate::store::is_conflict_name(name) {
            continue;
        }
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_dir() && deep {
            collect(root, &path, deep, out)?;
        } else if kind.is_file()
            && let Ok(rel) = path.strip_prefix(root)
            && let Some(rel) = rel.to_str()
        {
            out.push(rel.to_string());
        }
    }
    Ok(())
}

/// Whether `rel` is a project's evidence or raw file, shared as bytes.
fn is_file_rel(rel: &str) -> bool {
    matches!(
        rel.split('/').collect::<Vec<_>>().as_slice(),
        ["projects", _, "evidence" | "raw", _, ..]
    )
}

/// The project folder `rel` belongs to, None outside the projects.
fn project_dir(root: &Path, rel: &str) -> Option<PathBuf> {
    match rel.split('/').collect::<Vec<_>>().as_slice() {
        ["projects", project, _, ..] => Some(root.join("projects").join(project)),
        _ => None,
    }
}

/// Where a cloud session leaves its unsent writes, at the top of the checkout
/// it pushes.
pub const OUTBOX_FILE: &str = ".mem-outbox.json";

/// Saves the outbox, oldest first, into `top`'s outbox file and stages it;
/// with nothing waiting, takes an old file out of the tree and the index.
/// Returns how many writes it saved.
pub fn export(db: &Path, top: &Path, machine: &str) -> Result<usize> {
    let conn = open(db)?;
    let mut stmt = conn.prepare("SELECT id, path, base, body FROM outbox ORDER BY n")?;
    let writes = stmt
        .query_map([], |r| {
            Ok(json!({
                "id": r.get::<_, String>(0)?,
                "path": r.get::<_, String>(1)?,
                "base": r.get::<_, Option<String>>(2)?,
                "body": r.get::<_, Option<String>>(3)?,
            }))
        })?
        .collect::<rusqlite::Result<Vec<Value>>>()?;
    if writes.is_empty() {
        remove_outbox_file(top)?;
        return Ok(0);
    }
    let text = serde_json::to_string_pretty(&json!({"machine": machine, "writes": writes}))?;
    crate::atomic::write_replica(&top.join(OUTBOX_FILE), format!("{text}\n").as_bytes())?;
    // -f: a .gitignore naming the file must not keep it off the branch.
    git_in(top, &["add", "-f", "--", OUTBOX_FILE])?;
    Ok(writes.len())
}

/// Imports another machine's outbox file found at the top of the checkout
/// holding `cwd`, then deletes it and stages its removal. Each write keeps
/// its id, so the store re-acks one it already holds, and `imported`
/// remembers the ids so a file restored from history adds nothing. The
/// file of this machine's own export stays where it is.
pub fn import(db: &Path, root: &Path, machine: &str, cwd: &Path) -> Result<()> {
    // A walk for `.git` rather than asking git: most mem runs ask git nothing.
    let Some(top) = cwd
        .ancestors()
        .find(|d| d.join(".git").symlink_metadata().is_ok())
    else {
        return Ok(());
    };
    let Ok(text) = std::fs::read_to_string(top.join(OUTBOX_FILE)) else {
        return Ok(());
    };
    let saved: Value = serde_json::from_str(&text).context("not JSON")?;
    if saved["machine"].as_str().context("no machine")? == machine {
        return Ok(());
    }
    let mut writes = Vec::new();
    for write in saved["writes"].as_array().context("no writes")? {
        let text = |name: &str| match &write[name] {
            Value::Null => Ok(None),
            Value::String(s) => Ok(Some(s.clone())),
            _ => Err(anyhow::anyhow!("a write's {name} is not a string")),
        };
        let (Some(id), Some(rel)) = (text("id")?, text("path")?) else {
            anyhow::bail!("a write has no id or path");
        };
        anyhow::ensure!(is_record(&rel), "{rel} is not one of mem's records");
        let file =
            replica_path(root, &rel).with_context(|| format!("{rel} is not a project path"))?;
        writes.push((id, rel, file, text("base")?, text("body")?));
    }
    // A file speaks only for projects this machine knows, or one it brings
    // with its own project.toml: a branch is no licence to make others.
    for (_, rel, ..) in &writes {
        let project: String = rel.split('/').take(2).collect::<Vec<_>>().join("/");
        let brought = writes
            .iter()
            .any(|(_, other, ..)| *other == format!("{project}/project.toml"));
        anyhow::ensure!(
            brought || root.join(&project).join("project.toml").is_file(),
            "{rel} is in a project this machine does not know"
        );
    }
    let mut conn = open(db)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for (id, rel, file, base, body) in writes {
        if tx.execute("INSERT OR IGNORE INTO imported(id) VALUES (?1)", [&id])? == 0 {
            continue;
        }
        tx.execute(
            "INSERT OR IGNORE INTO outbox(id, path, base, body) VALUES (?1, ?2, ?3, ?4)",
            params![id, rel, base, body],
        )?;
        // The replica may already hold something newer than this write, so
        // nothing but a new item lands now and no head moves: the store's
        // ack says whether the write is the newest (see `apply`), and a
        // refusal turns a page into a conflict file.
        if is_item(&rel) && !file.exists() {
            land(&file, body.as_deref())?;
        }
    }
    tx.commit()?;
    remove_outbox_file(top)?;
    kick();
    Ok(())
}

/// Whether `rel` is where mem keeps a record of a project: its project.toml,
/// roadmap or plan, an item, a wiki page or a milestone plan. Bytes travel
/// on their own, never in an outbox file.
fn is_record(rel: &str) -> bool {
    let parts: Vec<&str> = rel.split('/').collect();
    let named = |name: &str| {
        name.strip_suffix(".md")
            .is_some_and(crate::store::is_valid_slug)
    };
    match parts.as_slice() {
        ["projects", _, "project.toml" | "roadmap.md" | "plan.md"] => true,
        ["projects", _, "items", name] => name.strip_suffix(".md").is_some_and(|id| {
            id.len() == 26
                && id
                    .chars()
                    .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
        }),
        ["projects", _, "wiki" | "plans", name] => named(name),
        _ => false,
    }
}

/// Deletes the outbox file and stages its removal.
fn remove_outbox_file(top: &Path) -> Result<()> {
    git_in(
        top,
        &[
            "rm",
            "-q",
            "--cached",
            "--ignore-unmatch",
            "--",
            OUTBOX_FILE,
        ],
    )?;
    land(&top.join(OUTBOX_FILE), None)
}

fn git_in(top: &Path, args: &[&str]) -> Result<()> {
    let out = std::process::Command::new("git")
        .current_dir(top)
        .args(args)
        .output()
        .context("running git")?;
    anyhow::ensure!(
        out.status.success(),
        "git {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(())
}
