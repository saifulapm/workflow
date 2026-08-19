//! The write verbs (spec §7). Every one of them lands on disk atomically, and
//! the singletons use compare-and-swap on mtime so two machines cannot silently
//! overwrite each other.

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use jiff::Timestamp;

use crate::app::App;
use crate::atomic::{read_mtime, write_atomic_cas};
use crate::exit;
use crate::ids::ShortIds;
use crate::item::{Item, Kind, Meta};
use crate::project::{Identity, Mode};
use crate::session;
use crate::store::Store;

/// `status.md` is capped at both a line count and a byte count (spec §4).
pub const STATUS_MAX_LINES: usize = 30;
pub const STATUS_MAX_BYTES: usize = 3_000;

/// Where a write for this identity goes. A non-git directory with no
/// `--project` writes to global scope — that is what global scope is for; a
/// project write from outside a checkout is what `--project` is for.
pub fn items_dir(store: &Store, identity: &Identity) -> PathBuf {
    match identity.id() {
        Some(id) => store.project_items(id),
        None => store.global_items(),
    }
}

/// The title of an item written from free text: its first line, trimmed.
pub fn derive_title(text: &str) -> String {
    let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let title = crate::search::truncate_bytes(first.trim(), 100);
    if title.is_empty() {
        "untitled".to_string()
    } else {
        title
    }
}

pub struct Written {
    pub id: String,
    pub short_id: String,
    pub path: PathBuf,
}

/// Mints an id, writes the item, and records the write against the session.
pub fn write_item(app: &App, identity: &Identity, mut meta: Meta, body: String) -> Result<Written> {
    // A store written by a newer mem is read-only here: a synced VERSION bump
    // must not let an older binary write a file the newer one cannot parse.
    crate::maint::check_write_version(&app.store)?;
    let dir = items_dir(&app.store, identity);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    crate::maint::ensure_version(&app.store)?;
    // Uniqueness is checked against the filesystem, not the index, and the
    // re-mint happens before anything is written: files are never renamed.
    let mut ids = ShortIds::scan(&app.store.root);
    meta.id = ids.mint().to_string();
    meta.project = identity.name().map(|n| n.to_string());
    let item = Item::new(meta, format!("\n{}\n", body.trim_end()).into_bytes());
    // An unwritable store spools rather than losing the write; doctor --fix
    // and the next successful write both replay the outbox.
    let path = match app.store.write_item(&dir, &item) {
        Ok(path) => {
            let _ = crate::maint::replay_outbox(&app.store, &app.dirs.outbox_dir());
            path
        }
        Err(e) => {
            let spooled = crate::maint::spool(&app.dirs.outbox_dir(), &item)
                .map_err(|_| exit::store_error(format!("could not write the item: {e}")))?;
            eprintln!(
                "mem: the store is unwritable ({e}); spooled to {} — replay with mem doctor --fix",
                spooled.display()
            );
            spooled
        }
    };
    if let Some(session) = &app.session_id {
        session::record_write(&app.dirs.sessions_dir(), session);
    }
    Ok(Written {
        short_id: item.meta.short_id(),
        id: item.meta.id,
        path,
    })
}

/// `mem save` / `mem log <text>` / `mem handoff --set` all end up here.
pub fn save(
    app: &App,
    kind: Kind,
    text: &str,
    title: Option<&str>,
    r#type: Option<&str>,
    tags: &[String],
    supersedes: Option<&str>,
) -> Result<Written> {
    let identity = app.identity(Mode::Write)?;
    let mut meta = Meta::new(
        String::new(),
        kind,
        title
            .map(|t| t.to_string())
            .unwrap_or_else(|| derive_title(text)),
        app.machine.clone(),
    );
    meta.r#type = r#type.map(|t| t.to_string());
    meta.tags = tags.to_vec();
    if let Some(sup) = supersedes {
        let Some(id_ref) = crate::ids::IdRef::parse(sup) else {
            return Err(exit::not_found(format!("'{sup}' is not an id")));
        };
        // Superseding an item mem cannot find would produce a dangling edge.
        let index = app.read_index()?;
        let mut rows = index.resolve_ref(&id_ref)?;
        match rows.len() {
            0 => return Err(exit::not_found(format!("no item {sup} to supersede"))),
            1 => meta.supersedes = Some(rows.remove(0).id),
            _ => {
                return Err(exit::coded(
                    exit::AMBIGUOUS,
                    format!("'{sup}' is ambiguous — use the full ULID"),
                ));
            }
        }
    }
    write_item(app, &identity, meta, text.to_string())
}

/// Reads `--stdin` content. Bytes that are not text are the caller holding it
/// wrong (exit 2), not a store that has gone wrong (exit 3) — an adapter that
/// saw a store error here would go hunting for a problem that is not there.
pub fn read_stdin() -> Result<String> {
    let mut buf = Vec::new();
    std::io::stdin()
        .read_to_end(&mut buf)
        .map_err(|e| exit::store_error(format!("reading stdin: {e}")))?;
    String::from_utf8(buf).map_err(|e| {
        exit::usage(format!(
            "stdin is not valid UTF-8 (byte {} is not)",
            e.utf8_error().valid_up_to()
        ))
    })
}

pub enum SingletonWrite {
    Written,
    /// Someone changed the file since it was read: no clobber (exit 5).
    Conflict,
    /// On disk, but over budget (exit 6).
    OverBudget {
        lines: usize,
        bytes: usize,
    },
}

/// Writes a singleton (`plan.md`, `status.md`) with CAS on its mtime, taking
/// the baseline now. Callers that spend time collecting their text — `--stdin`,
/// `--set-file` — read the baseline first and use `write_singleton_since`.
pub fn write_singleton(path: &Path, text: &str, cap: bool) -> Result<SingletonWrite> {
    write_singleton_since(path, text, cap, read_mtime(path))
}

/// The same write against a baseline the caller observed earlier. An over-cap
/// status still lands: refusing it would lose the content the caller already has
/// nowhere else to put.
pub fn write_singleton_since(
    path: &Path,
    text: &str,
    cap: bool,
    seen: Option<std::time::SystemTime>,
) -> Result<SingletonWrite> {
    let body = if text.ends_with('\n') {
        text.to_string()
    } else {
        format!("{text}\n")
    };
    if !write_atomic_cas(path, body.as_bytes(), seen)? {
        return Ok(SingletonWrite::Conflict);
    }
    if cap {
        let lines = body.lines().count();
        let bytes = body.len();
        if lines > STATUS_MAX_LINES || bytes > STATUS_MAX_BYTES {
            return Ok(SingletonWrite::OverBudget { lines, bytes });
        }
    }
    Ok(SingletonWrite::Written)
}

/// What one plan line had to say about a task id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tick {
    /// This line carries the id and was unchecked; here it is, checked.
    Flipped(String),
    /// This line carries the id and was already checked.
    Already,
    NoMatch,
}

/// One task line of the plan grammar: `- [ ] <id> <title>`. Everything after
/// the checkbox is preserved byte for byte — the title, the `[after: …]` tail
/// and any line ending — because a tick may only change the box.
pub fn tick_line(line: &str, id: &str) -> Tick {
    let indent_len = line.len() - line.trim_start().len();
    let (indent, rest) = line.split_at(indent_len);
    let Some(rest) = rest.strip_prefix("- ").or_else(|| rest.strip_prefix("* ")) else {
        return Tick::NoMatch;
    };
    let bullet = &line[indent_len..indent_len + 2];
    let (checked, tail) = if let Some(tail) = rest.strip_prefix("[ ] ") {
        (false, tail)
    } else if let Some(tail) = rest
        .strip_prefix("[x] ")
        .or_else(|| rest.strip_prefix("[X] "))
    {
        (true, tail)
    } else {
        return Tick::NoMatch;
    };
    // The id is the first token of the title, and only an exact one counts:
    // `t1` must never tick `t10`.
    if tail.split_whitespace().next() != Some(id) {
        return Tick::NoMatch;
    }
    if checked {
        return Tick::Already;
    }
    Tick::Flipped(format!("{indent}{bullet}[x] {tail}"))
}

/// The result of `mem plan --tick`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ticked {
    Flipped,
    /// Already checked: a tick is idempotent (spec §7).
    Already,
}

/// Ticks one task in `plan.md`. The read-modify-write is internal, so the CAS
/// is retried rather than surfaced: exit 5 is not a thing a caller can act on
/// here (spec §7).
pub fn tick_task(path: &Path, id: &str) -> Result<Ticked> {
    for _ in 0..3 {
        let seen = read_mtime(path);
        let text = std::fs::read_to_string(path)
            .map_err(|_| exit::not_found("no plan recorded for this project"))?;
        let mut lines: Vec<String> = text.split('\n').map(|l| l.to_string()).collect();
        let mut flipped = false;
        let mut found = false;
        for line in &mut lines {
            match tick_line(line, id) {
                Tick::Flipped(new) => {
                    *line = new;
                    found = true;
                    flipped = true;
                    break;
                }
                Tick::Already => {
                    found = true;
                    break;
                }
                Tick::NoMatch => {}
            }
        }
        if !found {
            return Err(exit::not_found(format!("no task '{id}' in the plan")));
        }
        if !flipped {
            return Ok(Ticked::Already);
        }
        if write_atomic_cas(path, lines.join("\n").as_bytes(), seen)? {
            return Ok(Ticked::Flipped);
        }
        // Someone rewrote the plan between the read and the write: read their
        // version and apply the tick to it instead.
    }
    Err(exit::coded(
        exit::CAS_CONFLICT,
        "plan.md is being rewritten faster than a tick can land — try again",
    ))
}

/// The project a write verb acts on, created if this is a git checkout mem has
/// not seen before (spec §5: only writes register).
pub fn writable_identity(app: &App) -> Result<Identity> {
    app.identity(Mode::Write)
}

/// `mem log` in read mode wants a lower bound; `--since` takes RFC3339 or
/// `<N>[mhd]`.
pub fn parse_since(text: &str) -> Result<Timestamp> {
    crate::timefmt::parse_since(text, Timestamp::now()).ok_or_else(|| {
        exit::usage(format!(
            "'{text}' is not a time — use RFC3339 or 90m, 4h, 2d"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tick_changes_the_box_and_nothing_else() {
        assert_eq!(
            tick_line("- [ ] t1 Extract pricing  [after: t0]", "t1"),
            Tick::Flipped("- [x] t1 Extract pricing  [after: t0]".to_string())
        );
        // A CRLF line ending survives, because it lives in the tail.
        assert_eq!(
            tick_line("- [ ] t1 Extract pricing\r", "t1"),
            Tick::Flipped("- [x] t1 Extract pricing\r".to_string())
        );
        assert_eq!(
            tick_line("  * [ ] t1 Indented and starred", "t1"),
            Tick::Flipped("  * [x] t1 Indented and starred".to_string())
        );
    }

    #[test]
    fn only_the_named_task_matches() {
        assert_eq!(
            tick_line("- [ ] t10 Delete the helper", "t1"),
            Tick::NoMatch
        );
        assert_eq!(tick_line("- [ ] t1 Extract pricing", "t10"), Tick::NoMatch);
        assert_eq!(tick_line("- [x] t1 Extract pricing", "t1"), Tick::Already);
        assert_eq!(tick_line("- [X] t1 Extract pricing", "t1"), Tick::Already);
        // Not task lines at all.
        assert_eq!(tick_line("# plan: cart-pricing-v2", "t1"), Tick::NoMatch);
        assert_eq!(tick_line("      Verify: t1 runs", "t1"), Tick::NoMatch);
        assert_eq!(tick_line("- t1 no checkbox", "t1"), Tick::NoMatch);
        assert_eq!(tick_line("", "t1"), Tick::NoMatch);
    }
}
