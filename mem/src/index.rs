//! The SQLite index (spec §6). Disposable: everything in it is derived from
//! the files in the store, so any doubt about its contents is resolved by
//! deleting it and reindexing.
//!
//! Two rules earn their own comments below because getting them wrong is
//! invisible rather than loud: the FTS5 table is `contentless_delete=1` and
//! every changed row must be DELETEd before it is re-INSERTed, and the reindex
//! transaction runs with `busy_timeout=0` so a read never waits on another
//! invocation's reindex.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use rusqlite::{
    Connection, ErrorCode, OptionalExtension, Transaction, TransactionBehavior, params,
};

use crate::ids::{IdRef, short_id};
use crate::item::Item;
use crate::project::Registry;
use crate::store::{Store, is_valid_slug, page_title, read_dir_sorted};

/// Bumped when the schema below changes; a mismatch rebuilds from scratch.
/// 2 added `idx_items_supersedes`, without which every read paid for an O(n²)
/// correlated subquery over `items.supersedes`.
/// 3 added the wiki pages, which get their own tables: a page is a document,
/// not an item, and nothing that counts or lists items should start seeing it.
/// 4 added a question's audience and task, so the hub can list what is a
/// person's to answer and a run can find what its worker asked.
const SCHEMA_VERSION: &str = "4";

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS items(
      rowid INTEGER PRIMARY KEY,
      id TEXT UNIQUE, short_id TEXT, project_id TEXT, project TEXT,
      kind TEXT, type TEXT, title TEXT, tags TEXT, machine TEXT,
      created_epoch INT, modified_epoch INT,
      path TEXT, size INT, mtime INT, ctime INT,
      supersedes TEXT, answers TEXT,
      active INT, superseded_by TEXT,
      archived INT, audience TEXT, task TEXT);
CREATE VIRTUAL TABLE IF NOT EXISTS items_fts USING fts5(
      title, body, tags,
      content='', contentless_delete=1,
      tokenize='porter unicode61');
CREATE TABLE IF NOT EXISTS pages(
      rowid INTEGER PRIMARY KEY,
      project_id TEXT, project TEXT, slug TEXT, title TEXT,
      path TEXT UNIQUE, size INT, mtime INT, ctime INT, modified_epoch INT);
CREATE VIRTUAL TABLE IF NOT EXISTS pages_fts USING fts5(
      title, body,
      content='', contentless_delete=1,
      tokenize='porter unicode61');
CREATE TABLE IF NOT EXISTS projects(id TEXT PRIMARY KEY, name TEXT, remote TEXT, aliases TEXT);
CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT);
CREATE INDEX IF NOT EXISTS idx_items_proj ON items(project_id, active, modified_epoch DESC);
CREATE INDEX IF NOT EXISTS idx_items_short ON items(short_id);
-- `recompute_derived` correlates every row against `supersedes`; without this
-- index that is a full scan per row, which at 1,000 items is 60 ms on every
-- read verb.
CREATE INDEX IF NOT EXISTS idx_items_supersedes ON items(supersedes);
CREATE INDEX IF NOT EXISTS idx_pages_proj ON pages(project_id, slug);
"#;

/// Why this connection was opened. Reads never wait on a lock; explicit writes
/// (`mem reindex`, `doctor --fix`) may.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    Read,
    Write,
}

impl Purpose {
    fn busy_timeout(self) -> Duration {
        match self {
            Purpose::Read => Duration::ZERO,
            Purpose::Write => Duration::from_millis(5000),
        }
    }
}

/// What one reindex pass did. Items and pages are counted together: what these
/// numbers mean is "rows brought up to date", and a user who just wrote a page
/// would read a count that left it out as "the page was not indexed".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reindex {
    pub indexed: usize,
    pub deleted: usize,
    pub unreadable: usize,
    /// Another invocation held the write lock, so the existing index stands.
    pub skipped: bool,
    /// The store root was absent, so nothing was treated as deleted.
    pub delete_phase_skipped: bool,
}

/// One indexed item, as the read verbs see it.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub id: String,
    pub short_id: String,
    pub project_id: Option<String>,
    pub project: Option<String>,
    pub kind: String,
    pub r#type: Option<String>,
    pub title: String,
    pub tags: Vec<String>,
    pub machine: String,
    pub created_epoch: i64,
    pub modified_epoch: i64,
    pub path: PathBuf,
    pub supersedes: Option<String>,
    pub answers: Option<String>,
    pub active: bool,
    pub superseded_by: Option<String>,
    pub archived: bool,
    pub audience: Option<String>,
    pub task: Option<String>,
}

/// Qualified with the table name: `items_fts` also has `title` and `tags`, so
/// an unqualified list is ambiguous the moment the two are joined.
pub const ROW_COLUMNS: &str = "items.id, items.short_id, items.project_id, items.project, \
     items.kind, items.type, items.title, items.tags, items.machine, items.created_epoch, \
     items.modified_epoch, items.path, items.supersedes, items.answers, items.active, \
     items.superseded_by, items.archived, items.audience, items.task";

/// The `kind` a page wears. Pages come back from a search through the same
/// `Row` as items, so that one query can rank both, and this is what tells
/// them apart — no item has this kind.
pub const WIKI_KIND: &str = "wiki";

/// The page columns under the `Row` names, so `Row::from_sql` reads a page as
/// happily as an item. A page has no ULID, no machine and no supersede edge:
/// its id says what it is, and its short id is its slug.
pub fn page_row_columns() -> String {
    format!(
        "('wiki:' || pages.project_id || '/' || pages.slug) AS id, pages.slug AS short_id, \
         pages.project_id, pages.project, '{WIKI_KIND}' AS kind, NULL AS type, pages.title, \
         '[]' AS tags, '' AS machine, pages.modified_epoch AS created_epoch, \
         pages.modified_epoch, pages.path, NULL AS supersedes, NULL AS answers, 1 AS active, \
         NULL AS superseded_by, 0 AS archived, NULL AS audience, NULL AS task"
    )
}

impl Row {
    /// The slug, when this row is a page rather than an item.
    pub fn wiki_slug(&self) -> Option<&str> {
        (self.kind == WIKI_KIND).then_some(self.short_id.as_str())
    }

    pub fn from_sql(row: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
        let tags: String = row.get("tags")?;
        Ok(Row {
            id: row.get("id")?,
            short_id: row.get("short_id")?,
            project_id: row.get("project_id")?,
            project: row.get("project")?,
            kind: row.get("kind")?,
            r#type: row.get("type")?,
            title: row.get("title")?,
            tags: serde_json::from_str(&tags).unwrap_or_default(),
            machine: row.get("machine")?,
            created_epoch: row.get("created_epoch")?,
            modified_epoch: row.get("modified_epoch")?,
            path: PathBuf::from(row.get::<_, String>("path")?),
            supersedes: row.get("supersedes")?,
            answers: row.get("answers")?,
            active: row.get::<_, i64>("active")? != 0,
            superseded_by: row.get("superseded_by")?,
            archived: row.get::<_, i64>("archived")? != 0,
            audience: row.get("audience")?,
            task: row.get("task")?,
        })
    }
}

pub struct Index {
    pub conn: Connection,
    purpose: Purpose,
}

impl Index {
    /// Opens (creating or rebuilding as needed) the index. A file that is not a
    /// database, or one from an older schema, is deleted and recreated: the
    /// index is derived data, so rebuilding costs nothing but time.
    pub fn open(path: &Path, purpose: Purpose) -> Result<Index> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        match Index::try_open(path, purpose) {
            Ok(index) => Ok(index),
            Err(_) => {
                let _ = std::fs::remove_file(path);
                let _ = std::fs::remove_file(path.with_extension("db-wal"));
                let _ = std::fs::remove_file(path.with_extension("db-shm"));
                Index::try_open(path, purpose).context("rebuilding the index")
            }
        }
    }

    fn try_open(path: &Path, purpose: Purpose) -> Result<Index> {
        let mut conn =
            Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        // Schema creation is a one-off write; waiting a moment for it beats
        // failing when two fresh invocations start at the same instant. The
        // reindex transaction resets this to the purpose's own timeout.
        conn.busy_timeout(Duration::from_millis(1000))?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .context("enabling WAL")?;
        assert_capabilities(&conn)?;
        ensure_schema(&conn)?;
        conn.busy_timeout(purpose.busy_timeout())?;
        // Every transaction mem opens is a reindex, and a reindex must take the
        // write lock at BEGIN rather than upgrading half way through.
        conn.set_transaction_behavior(TransactionBehavior::Immediate);
        Ok(Index { conn, purpose })
    }

    pub fn purpose(&self) -> Purpose {
        self.purpose
    }

    pub fn has_fts5(&self) -> Result<bool> {
        Ok(has_fts5(&self.conn)?)
    }

    pub fn sqlite_version(&self) -> Result<(u32, u32, u32)> {
        Ok(sqlite_version(&self.conn)?)
    }

    /// Takes the index write lock, for tests and for callers that want to hold
    /// off other invocations. Returns None when someone else holds it.
    pub fn begin_immediate(&mut self) -> Option<Transaction<'_>> {
        self.conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .ok()
    }

    pub fn count_items(&self) -> Result<usize> {
        Ok(self
            .conn
            .query_row("SELECT count(*) FROM items", [], |r| r.get::<_, i64>(0))?
            as usize)
    }

    pub fn count_active(&self) -> Result<usize> {
        Ok(self
            .conn
            .query_row("SELECT count(*) FROM items WHERE active = 1", [], |r| {
                r.get::<_, i64>(0)
            })? as usize)
    }

    pub fn count_pages(&self) -> Result<usize> {
        Ok(self
            .conn
            .query_row("SELECT count(*) FROM pages", [], |r| r.get::<_, i64>(0))?
            as usize)
    }

    /// How many items match a token. Counting FTS rows rather than item rows is
    /// what makes a corrupt index visible: duplicate FTS rows and orphaned FTS
    /// rows both show up here and in neither `count_items` nor a rowcount.
    pub fn fts_match_count(&self, token: &str) -> Result<usize> {
        let sql = "SELECT count(*) FROM items_fts WHERE items_fts MATCH ?1";
        Ok(self
            .conn
            .query_row(sql, params![token], |r| r.get::<_, i64>(0))? as usize)
    }

    /// The same count on the page side.
    pub fn pages_fts_match_count(&self, token: &str) -> Result<usize> {
        let sql = "SELECT count(*) FROM pages_fts WHERE pages_fts MATCH ?1";
        Ok(self
            .conn
            .query_row(sql, params![token], |r| r.get::<_, i64>(0))? as usize)
    }

    pub fn get(&self, id: &str) -> Result<Option<Row>> {
        let sql = format!("SELECT {ROW_COLUMNS} FROM items WHERE id = ?1");
        Ok(self
            .conn
            .query_row(&sql, params![id], Row::from_sql)
            .optional()?)
    }

    /// Resolves a full ULID or an exact 8-character suffix. More than one row
    /// back means the suffix is ambiguous, which only a sync merge can cause.
    pub fn resolve_ref(&self, id_ref: &IdRef) -> Result<Vec<Row>> {
        let (sql, arg) = match id_ref {
            IdRef::Full(id) => (
                format!("SELECT {ROW_COLUMNS} FROM items WHERE items.id = ?1"),
                id.clone(),
            ),
            IdRef::Short(short) => (
                format!("SELECT {ROW_COLUMNS} FROM items WHERE items.short_id = ?1"),
                short.clone(),
            ),
        };
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![arg], Row::from_sql)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn count_for_project(&self, project_id: &str) -> Result<usize> {
        Ok(self.conn.query_row(
            "SELECT count(*) FROM items WHERE project_id = ?1",
            params![project_id],
            |r| r.get::<_, i64>(0),
        )? as usize)
    }

    /// Rows of one kind, newest first.
    pub fn recent(&self, kind: &str, project_id: Option<&str>, limit: usize) -> Result<Vec<Row>> {
        let mut sql =
            format!("SELECT {ROW_COLUMNS} FROM items WHERE items.active = 1 AND items.kind = ?1");
        match project_id {
            Some(_) => sql.push_str(" AND items.project_id = ?2"),
            None => sql.push_str(" AND items.project_id IS NULL"),
        }
        sql.push_str(" ORDER BY items.modified_epoch DESC, items.id DESC LIMIT ");
        sql.push_str(&limit.to_string());
        let mut stmt = self.conn.prepare(&sql)?;
        let rows: rusqlite::Result<Vec<Row>> = match project_id {
            Some(p) => stmt.query_map(params![kind, p], Row::from_sql)?.collect(),
            None => stmt.query_map(params![kind], Row::from_sql)?.collect(),
        };
        Ok(rows?)
    }

    /// Questions nothing has answered yet, oldest first — the order they should
    /// be dealt with in.
    pub fn pending_questions(&self, project_id: Option<&str>) -> Result<Vec<Row>> {
        let mut sql = format!(
            "SELECT {ROW_COLUMNS} FROM items
             WHERE items.active = 1 AND items.kind = 'question'
               AND NOT EXISTS (SELECT 1 FROM items a WHERE a.answers = items.id)"
        );
        match project_id {
            Some(_) => sql.push_str(" AND items.project_id = ?1"),
            None => sql.push_str(" AND items.project_id IS NULL"),
        }
        sql.push_str(" ORDER BY items.created_epoch ASC, items.id ASC");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows: rusqlite::Result<Vec<Row>> = match project_id {
            Some(p) => stmt.query_map(params![p], Row::from_sql)?.collect(),
            None => stmt.query_map([], Row::from_sql)?.collect(),
        };
        Ok(rows?)
    }

    /// The answer to a question, if one exists.
    pub fn answer_to(&self, question_id: &str) -> Result<Option<Row>> {
        let sql = format!("SELECT {ROW_COLUMNS} FROM items WHERE items.answers = ?1 LIMIT 1");
        Ok(self
            .conn
            .query_row(&sql, params![question_id], Row::from_sql)
            .optional()?)
    }

    /// Items with more than one replacement: two machines superseded the same
    /// item differently. Both stay active; doctor reports the fork.
    pub fn supersede_forks(&self) -> Result<Vec<(String, Vec<String>)>> {
        let mut stmt = self.conn.prepare(
            "SELECT supersedes, group_concat(id, ' ') FROM items
             WHERE supersedes IS NOT NULL AND supersedes <> ''
             GROUP BY supersedes HAVING count(*) > 1",
        )?;
        let rows = stmt.query_map([], |r| {
            let target: String = r.get(0)?;
            let ids: String = r.get(1)?;
            Ok((
                target,
                ids.split(' ').map(|s| s.to_string()).collect::<Vec<_>>(),
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Brings the index up to date with the store. One `BEGIN IMMEDIATE`: if
    /// another invocation is already reindexing, this returns immediately with
    /// `skipped` and the caller serves the existing, possibly stale, index.
    pub fn reindex(&self, store: &Store, full: bool) -> Result<Reindex> {
        let tx = match self.conn.unchecked_transaction() {
            Ok(tx) => tx,
            Err(e) if is_busy(&e) => {
                return Ok(Reindex {
                    skipped: true,
                    ..Default::default()
                });
            }
            Err(e) => return Err(e).context("starting the reindex transaction"),
        };
        let outcome = reindex_in(&tx, store, full)?;
        tx.commit().context("committing the reindex")?;
        Ok(outcome)
    }
}

fn is_busy(err: &rusqlite::Error) -> bool {
    matches!(
        err.sqlite_error_code(),
        Some(ErrorCode::DatabaseBusy) | Some(ErrorCode::DatabaseLocked)
    )
}

fn has_fts5(conn: &Connection) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT 1 FROM pragma_compile_options WHERE compile_options='ENABLE_FTS5'",
        [],
        |r| r.get::<_, i64>(0),
    )
    .optional()
    .map(|v| v.is_some())
}

fn sqlite_version(conn: &Connection) -> rusqlite::Result<(u32, u32, u32)> {
    let text: String = conn.query_row("SELECT sqlite_version()", [], |r| r.get(0))?;
    let mut parts = text.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    Ok((
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    ))
}

/// Startup assertions from spec §6. `contentless_delete=1` is what makes the
/// FTS side deletable at all, and it needs SQLite 3.43.
fn assert_capabilities(conn: &Connection) -> Result<()> {
    if !has_fts5(conn)? {
        bail!("this build of SQLite has no FTS5; mem cannot index without it");
    }
    let (major, minor, patch) = sqlite_version(conn)?;
    if (major, minor) < (3, 43) {
        bail!("SQLite {major}.{minor}.{patch} is too old — contentless_delete needs 3.43 or newer");
    }
    Ok(())
}

fn ensure_schema(conn: &Connection) -> Result<()> {
    let existing: Option<String> = conn
        .query_row("SELECT value FROM meta WHERE key='schema'", [], |r| {
            r.get(0)
        })
        .optional()
        .unwrap_or(None);
    match existing.as_deref() {
        Some(SCHEMA_VERSION) => Ok(()),
        Some(_) => {
            // Derived data: throwing it away is always safe.
            conn.execute_batch(
                "DROP TABLE IF EXISTS items; DROP TABLE IF EXISTS items_fts;
                 DROP TABLE IF EXISTS pages; DROP TABLE IF EXISTS pages_fts;
                 DROP TABLE IF EXISTS projects; DROP TABLE IF EXISTS meta;",
            )?;
            create_schema(conn)
        }
        None => create_schema(conn),
    }
}

fn create_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(SCHEMA).context("creating the schema")?;
    conn.execute(
        "INSERT OR REPLACE INTO meta(key, value) VALUES('schema', ?1)",
        params![SCHEMA_VERSION],
    )?;
    Ok(())
}

/// (mtime, size, ctime) — the change predicate. ctime is what catches an edit
/// that keeps the size and is written inside one mtime tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    size: i64,
    mtime: i64,
    ctime: i64,
}

impl Stamp {
    /// The mtime in whole seconds, which is the resolution dates are shown at.
    fn mtime_seconds(self) -> i64 {
        self.mtime / 1_000_000_000
    }
}

fn stamp(meta: &std::fs::Metadata) -> Stamp {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Stamp {
            size: meta.len() as i64,
            mtime: meta.mtime() * 1_000_000_000 + meta.mtime_nsec(),
            ctime: meta.ctime() * 1_000_000_000 + meta.ctime_nsec(),
        }
    }
    #[cfg(not(unix))]
    {
        let secs = |t: std::io::Result<std::time::SystemTime>| -> i64 {
            t.ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos() as i64)
                .unwrap_or(0)
        };
        Stamp {
            size: meta.len() as i64,
            mtime: secs(meta.modified()),
            ctime: secs(meta.created()),
        }
    }
}

fn reindex_in(tx: &Transaction<'_>, store: &Store, full: bool) -> Result<Reindex> {
    let mut outcome = Reindex::default();

    let mut known: HashMap<String, (i64, Stamp)> = HashMap::new();
    {
        let mut stmt = tx.prepare("SELECT rowid, path, size, mtime, ctime FROM items")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(1)?,
                (
                    r.get::<_, i64>(0)?,
                    Stamp {
                        size: r.get(2)?,
                        mtime: r.get(3)?,
                        ctime: r.get(4)?,
                    },
                ),
            ))
        })?;
        for row in rows {
            let (path, entry) = row?;
            known.insert(path, entry);
        }
    }

    let registry = Registry::load(store);
    let store_present = store.exists();
    let files = store.item_paths();

    tx.execute(
        "CREATE TEMP TABLE IF NOT EXISTS seen(path TEXT PRIMARY KEY)",
        [],
    )?;
    tx.execute("DELETE FROM seen", [])?;

    for path in &files {
        let path_text = path.to_string_lossy().to_string();
        let Ok(meta) = std::fs::metadata(path) else {
            continue;
        };
        let current = stamp(&meta);
        tx.execute(
            "INSERT OR IGNORE INTO seen(path) VALUES(?1)",
            params![path_text],
        )?;

        let previous = known.get(&path_text).copied();
        if !full && previous.is_some_and(|(_, s)| s == current) {
            continue;
        }

        let item = match crate::store::read_item(path) {
            Ok(item) => item,
            Err(_) => {
                // A half-synced or hand-mangled file must not fail the whole
                // reindex; doctor reports it.
                outcome.unreadable += 1;
                continue;
            }
        };

        if let Some((rowid, _)) = previous {
            delete_row(tx, rowid)?;
        }
        insert_row(tx, store, &registry, path, &path_text, current, &item)?;
        outcome.indexed += 1;
    }

    // A store root that is not there yet is not a store full of deletions.
    if store_present {
        let stale: Vec<i64> = {
            let mut stmt =
                tx.prepare("SELECT rowid FROM items WHERE path NOT IN (SELECT path FROM seen)")?;
            let rows = stmt.query_map([], |r| r.get::<_, i64>(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for rowid in stale {
            delete_row(tx, rowid)?;
            outcome.deleted += 1;
        }
    } else {
        outcome.delete_phase_skipped = true;
    }

    // `active` and `superseded_by` derive from the supersedes edges and the
    // archived flags, both of which arrive with a row. A pass that inserted and
    // deleted nothing cannot have changed either, and this runs before every
    // single read. Pages come after it: they have no derived columns, and
    // folding their counts in first would run it for nothing.
    if outcome.indexed > 0 || outcome.deleted > 0 {
        recompute_derived(tx)?;
    }
    reindex_pages(tx, store, &registry, full, store_present, &mut outcome)?;
    sync_projects(tx, &registry)?;
    Ok(outcome)
}

/// The FTS row goes first and by rowid: a contentless FTS5 table cannot be
/// deleted from by value, and an INSERT that reuses a live rowid silently
/// accumulates a second copy while the item count stays right.
fn delete_row(tx: &Transaction<'_>, rowid: i64) -> Result<()> {
    tx.execute("DELETE FROM items_fts WHERE rowid = ?1", params![rowid])?;
    tx.execute("DELETE FROM items WHERE rowid = ?1", params![rowid])?;
    Ok(())
}

fn insert_row(
    tx: &Transaction<'_>,
    store: &Store,
    registry: &Registry,
    path: &Path,
    path_text: &str,
    stamp: Stamp,
    item: &Item,
) -> Result<()> {
    let project_id = project_id_of(store, path);
    let project = item.meta.project.clone().or_else(|| {
        project_id
            .as_deref()
            .and_then(|id| registry.by_id(id))
            .map(|p| p.name.clone())
    });
    let tags = serde_json::to_string(&item.meta.tags)?;
    tx.execute(
        "INSERT INTO items(id, short_id, project_id, project, kind, type, title, tags, machine,
                           created_epoch, modified_epoch, path, size, mtime, ctime,
                           supersedes, answers, active, superseded_by, archived,
                           audience, task)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,1,NULL,?18,
                ?19,?20)",
        params![
            item.meta.id,
            short_id(&item.meta.id),
            project_id,
            project,
            item.meta.kind.as_str(),
            item.meta.r#type,
            item.meta.title,
            tags,
            item.meta.machine,
            item.meta.created.as_second(),
            item.meta.modified.as_second(),
            path_text,
            stamp.size,
            stamp.mtime,
            stamp.ctime,
            item.meta.supersedes,
            item.meta.answers,
            i64::from(item.meta.is_archived()),
            item.meta.audience,
            item.meta.task,
        ],
    )?;
    let rowid = tx.last_insert_rowid();
    tx.execute(
        "INSERT INTO items_fts(rowid, title, body, tags) VALUES(?1, ?2, ?3, ?4)",
        params![
            rowid,
            item.meta.title,
            item.body_str(),
            item.meta.tags.join(" ")
        ],
    )?;
    Ok(())
}

/// `…/projects/<id>/items/<ulid>.md` → `<id>`; the global tree has none.
fn project_id_of(store: &Store, path: &Path) -> Option<String> {
    let parent = path.parent()?.parent()?;
    if parent.parent() != Some(store.projects_dir().as_path()) {
        return None;
    }
    parent.file_name().map(|n| n.to_string_lossy().to_string())
}

/// One page file, as the scan finds it.
struct PageFile {
    project_id: String,
    slug: String,
    path: PathBuf,
}

/// `<root>/projects/<id>/wiki/<slug>.md`. Anything else in a wiki directory — a
/// dot-temp, a bisync conflict loser, a name that is not a slug, a directory —
/// is skipped the way a stray beside an item is.
fn page_files(store: &Store) -> Vec<PageFile> {
    let mut out = Vec::new();
    for project in read_dir_sorted(&store.projects_dir()) {
        let Some(project_id) = project.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        for path in read_dir_sorted(&store.wiki_dir(project_id)) {
            let slug = path
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(".md"));
            let Some(slug) = slug.filter(|s| is_valid_slug(s)) else {
                continue;
            };
            if !path.is_file() {
                continue;
            }
            out.push(PageFile {
                project_id: project_id.to_string(),
                slug: slug.to_string(),
                path: path.clone(),
            });
        }
    }
    out
}

/// The item pass over again for pages: same change predicate, same delete
/// phase, its own tables. The text is read only for a page that changed, so a
/// wiki costs nothing on the reads that run a reindex first.
fn reindex_pages(
    tx: &Transaction<'_>,
    store: &Store,
    registry: &Registry,
    full: bool,
    store_present: bool,
    outcome: &mut Reindex,
) -> Result<()> {
    let mut known: HashMap<String, (i64, Stamp)> = HashMap::new();
    {
        let mut stmt = tx.prepare("SELECT rowid, path, size, mtime, ctime FROM pages")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(1)?,
                (
                    r.get::<_, i64>(0)?,
                    Stamp {
                        size: r.get(2)?,
                        mtime: r.get(3)?,
                        ctime: r.get(4)?,
                    },
                ),
            ))
        })?;
        for row in rows {
            let (path, entry) = row?;
            known.insert(path, entry);
        }
    }

    tx.execute(
        "CREATE TEMP TABLE IF NOT EXISTS seen_pages(path TEXT PRIMARY KEY)",
        [],
    )?;
    tx.execute("DELETE FROM seen_pages", [])?;

    for page in page_files(store) {
        let path_text = page.path.to_string_lossy().to_string();
        let Ok(meta) = std::fs::metadata(&page.path) else {
            continue;
        };
        let current = stamp(&meta);
        tx.execute(
            "INSERT OR IGNORE INTO seen_pages(path) VALUES(?1)",
            params![path_text],
        )?;

        let previous = known.get(&path_text).copied();
        if !full && previous.is_some_and(|(_, s)| s == current) {
            continue;
        }

        // A page is markdown a person wrote; anything that is not text is as
        // unreadable as a mangled item, and doctor reports it.
        let Ok(text) = std::fs::read_to_string(&page.path) else {
            outcome.unreadable += 1;
            continue;
        };

        if let Some((rowid, _)) = previous {
            delete_page_row(tx, rowid)?;
        }
        insert_page_row(tx, registry, &page, &path_text, current, &text)?;
        outcome.indexed += 1;
    }

    if store_present {
        let stale: Vec<i64> = {
            let mut stmt = tx.prepare(
                "SELECT rowid FROM pages WHERE path NOT IN (SELECT path FROM seen_pages)",
            )?;
            let rows = stmt.query_map([], |r| r.get::<_, i64>(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for rowid in stale {
            delete_page_row(tx, rowid)?;
            outcome.deleted += 1;
        }
    }
    Ok(())
}

/// The FTS row first and by rowid, for the reason `delete_row` gives.
fn delete_page_row(tx: &Transaction<'_>, rowid: i64) -> Result<()> {
    tx.execute("DELETE FROM pages_fts WHERE rowid = ?1", params![rowid])?;
    tx.execute("DELETE FROM pages WHERE rowid = ?1", params![rowid])?;
    Ok(())
}

fn insert_page_row(
    tx: &Transaction<'_>,
    registry: &Registry,
    page: &PageFile,
    path_text: &str,
    stamp: Stamp,
    text: &str,
) -> Result<()> {
    // A page carries no frontmatter, so its title is its first heading and its
    // date is the file's.
    let title = page_title(text);
    tx.execute(
        "INSERT INTO pages(project_id, project, slug, title, path, size, mtime, ctime,
                           modified_epoch)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![
            page.project_id,
            registry.by_id(&page.project_id).map(|p| p.name.clone()),
            page.slug,
            title,
            path_text,
            stamp.size,
            stamp.mtime,
            stamp.ctime,
            stamp.mtime_seconds(),
        ],
    )?;
    let rowid = tx.last_insert_rowid();
    tx.execute(
        "INSERT INTO pages_fts(rowid, title, body) VALUES(?1, ?2, ?3)",
        params![rowid, title, text],
    )?;
    Ok(())
}

/// `active` and `superseded_by` are derived, never stored in a file: an item is
/// inactive when something supersedes it or when it has been archived.
fn recompute_derived(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "UPDATE items SET superseded_by = (
             SELECT s.id FROM items s
             WHERE s.supersedes = items.id AND s.id <> items.id
             ORDER BY s.created_epoch DESC, s.id DESC LIMIT 1);
         UPDATE items SET active = CASE
             WHEN archived = 1 OR superseded_by IS NOT NULL THEN 0 ELSE 1 END;",
    )?;
    Ok(())
}

fn sync_projects(tx: &Transaction<'_>, registry: &Registry) -> Result<()> {
    tx.execute("DELETE FROM projects", [])?;
    for p in &registry.projects {
        tx.execute(
            "INSERT OR REPLACE INTO projects(id, name, remote, aliases) VALUES(?1,?2,?3,?4)",
            params![p.id, p.name, p.remote, serde_json::to_string(&p.aliases)?],
        )?;
    }
    Ok(())
}
