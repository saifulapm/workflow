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
    let Some((rel, false)) = shared(root, path) else {
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
    let Some((rel, true)) = shared(&ctx.root, path) else {
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
    let Some((rel, true)) = shared(&ctx.root, path) else {
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
/// (shared as a write). Temps and conflict copies are never shared.
fn shared(root: &Path, path: &Path) -> Option<(String, bool)> {
    let rel = path.strip_prefix(root).ok()?;
    let parts: Vec<&str> = rel.iter().map(|p| p.to_str()).collect::<Option<_>>()?;
    let [first, project, rest @ ..] = parts.as_slice() else {
        return None;
    };
    let name = rest.last()?;
    if *first != "projects" || name.starts_with('.') || crate::store::is_conflict_name(name) {
        return None;
    }
    let store = crate::store::Store::new(root.to_path_buf());
    let bytes = matches!(rest.first(), Some(&"evidence" | &"raw"));
    (crate::project::declared(&store, project, "share").as_deref() == Some("true"))
        .then(|| (parts.join("/"), bytes))
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
         CREATE TABLE IF NOT EXISTS files(n INTEGER PRIMARY KEY AUTOINCREMENT, path TEXT NOT NULL UNIQUE);
         CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value INTEGER NOT NULL);",
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
    Ok(json!({"t": "hello", "machine": machine, "since": seq(&conn)?}))
}

/// The outbox, oldest first, as the store's `write` messages, then each file
/// still to upload as `{"t":"file","path","local"}`, `local` being where the
/// bytes are on this machine.
pub fn pending(db: &Path, root: &Path) -> Result<Vec<Value>> {
    let conn = open(db)?;
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
    let mut out: Vec<Value> = rows.collect::<rusqlite::Result<_>>()?;
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
pub fn apply(db: &Path, root: &Path, message: &Value) -> Result<()> {
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
            if shared(root, &file).is_none() {
                raise_seq(&tx, seen)?;
                tx.commit()?;
                return Ok(());
            }
            // A local write still waiting wins: the store answers it with an
            // ack, or with a refusal that carries this record's version.
            if head(&tx, &rel)?.as_deref() != Some(id.as_str()) && !waiting_for(&tx, &rel)? {
                land(&file, message["body"].as_str())?;
                set_head(&tx, &rel, &id)?;
            }
            raise_seq(&tx, seen)?;
        }
        Some("ack") => {
            let Some(id) = text(&message["id"]) else {
                return Ok(());
            };
            if let Some((rel, body)) = take(&tx, &id)? {
                // The replica missed this write when mem died between the
                // outbox and the rename; the ack is the moment to heal it.
                if head(&tx, &rel)?.as_deref() == Some(id.as_str())
                    && !waiting_for(&tx, &rel)?
                    && let Some(file) = replica_path(root, &rel)
                    && std::fs::read_to_string(&file).ok() != body
                {
                    land(&file, body.as_deref())?;
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
        }
        _ => {}
    }
    tx.commit()?;
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
/// leave the projects directory.
fn replica_path(root: &Path, rel: &str) -> Option<PathBuf> {
    let rel = Path::new(rel);
    let mut parts = rel.components();
    if parts.next() != Some(Component::Normal("projects".as_ref())) {
        return None;
    }
    if !rel.components().all(|c| matches!(c, Component::Normal(_))) || parts.count() < 2 {
        return None;
    }
    Some(root.join(rel))
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
