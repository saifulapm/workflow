//! Incremental reindex, delete detection and lock behaviour (spec §6, AC7–AC9).

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::{World, item, put};
use mem::index::{Index, Purpose, Reindex};
use mem::item::Kind;
use mem::store::Store;

fn open_read(w: &World) -> Index {
    Index::open(&w.index_path(), Purpose::Read).expect("open index")
}

fn reindex(w: &World) -> Reindex {
    let index = open_read(w);
    index.reindex(&w.store(), false).expect("reindex")
}

/// Rows whose FTS side actually matches, which is the only count that can tell
/// a healthy index from one carrying duplicate FTS rows.
fn fts_hits(index: &Index, token: &str) -> usize {
    index.fts_match_count(token).expect("fts count")
}

/// The same count on the page side.
fn page_hits(index: &Index, token: &str) -> usize {
    index.pages_fts_match_count(token).expect("page fts count")
}

/// Writes a page where `mem wiki --stdin` would put it.
fn page(store: &Store, project_id: &str, slug: &str, text: &str) -> PathBuf {
    let dir = store.wiki_dir(project_id);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{slug}.md"));
    std::fs::write(&path, text).unwrap();
    path
}

#[test]
fn a_fresh_index_holds_every_item_in_the_store() {
    let w = World::new("idx-fresh");
    let p = w.project("01K2AAAAAAAAAAAAAAAAAAAAAA", "thing");
    let store = w.store();
    put(
        &store,
        Some(&p),
        &item(Kind::Fact, "redis sessions", "we use redis"),
    );
    put(
        &store,
        Some(&p),
        &item(Kind::Log, "did a thing", "log body"),
    );
    put(
        &store,
        None,
        &item(Kind::Fact, "global fact", "global body"),
    );

    let outcome = reindex(&w);
    assert_eq!(outcome.indexed, 3);
    assert_eq!(outcome.deleted, 0);

    let index = open_read(&w);
    assert_eq!(index.count_items().unwrap(), 3);
    assert_eq!(fts_hits(&index, "redis"), 1);

    // A second pass with nothing changed reindexes nothing and keeps the FTS
    // side exactly as big as the items side.
    let again = index.reindex(&store, false).unwrap();
    assert_eq!(again.indexed, 0);
    assert_eq!(fts_hits(&index, "redis"), 1);
}

#[test]
fn a_deleted_file_leaves_no_row_and_no_fts_hit() {
    let w = World::new("idx-delete");
    let p = w.project("01K2AAAAAAAAAAAAAAAAAAAAAA", "thing");
    let store = w.store();
    let keep = item(Kind::Fact, "kept", "keepertoken lives on");
    let gone = item(Kind::Fact, "removed", "vanishingtoken should disappear");
    put(&store, Some(&p), &keep);
    let gone_path = put(&store, Some(&p), &gone);

    reindex(&w);
    let index = open_read(&w);
    assert_eq!(fts_hits(&index, "vanishingtoken"), 1);

    std::fs::remove_file(&gone_path).unwrap();
    let outcome = index.reindex(&store, false).unwrap();
    assert_eq!(outcome.deleted, 1);
    assert_eq!(index.count_items().unwrap(), 1);
    assert_eq!(
        fts_hits(&index, "vanishingtoken"),
        0,
        "the FTS row must go with the item row"
    );
    assert_eq!(fts_hits(&index, "keepertoken"), 1);
}

#[test]
fn a_same_size_edit_is_detected_and_the_old_text_stops_matching() {
    let w = World::new("idx-edit");
    let p = w.project("01K2AAAAAAAAAAAAAAAAAAAAAA", "thing");
    let store = w.store();
    let mut it = item(Kind::Fact, "title", "alphatoken is here");
    let path = put(&store, Some(&p), &it);
    reindex(&w);
    let index = open_read(&w);
    assert_eq!(fts_hits(&index, "alphatoken"), 1);

    // Same length, same mtime: only ctime moves. A change predicate that
    // ignored ctime would miss this edit entirely.
    let before = std::fs::metadata(&path).unwrap();
    it.body = b"betatoken is here!".to_vec();
    assert_eq!(it.body.len(), b"alphatoken is here".len());
    store.write_item(&store.project_items(&p), &it).unwrap();
    let f = std::fs::File::options().write(true).open(&path).unwrap();
    f.set_times(
        std::fs::FileTimes::new()
            .set_modified(before.modified().unwrap())
            .set_accessed(before.accessed().unwrap()),
    )
    .unwrap();
    drop(f);
    let after = std::fs::metadata(&path).unwrap();
    assert_eq!(
        after.len(),
        before.len(),
        "the test needs an equal-size edit"
    );
    assert_eq!(
        after.modified().unwrap(),
        before.modified().unwrap(),
        "the test needs an equal-mtime edit"
    );

    let outcome = index.reindex(&store, false).unwrap();
    assert_eq!(
        outcome.indexed, 1,
        "an equal-size, equal-mtime edit is a change"
    );
    assert_eq!(
        fts_hits(&index, "alphatoken"),
        0,
        "the pre-edit text must stop matching — a row count cannot see this"
    );
    assert_eq!(fts_hits(&index, "betatoken"), 1);
    assert_eq!(index.count_items().unwrap(), 1);
}

#[test]
fn an_absent_store_root_never_empties_the_index() {
    let w = World::new("idx-absent");
    let p = w.project("01K2AAAAAAAAAAAAAAAAAAAAAA", "thing");
    let store = w.store();
    put(&store, Some(&p), &item(Kind::Fact, "a", "survivortoken"));
    put(&store, Some(&p), &item(Kind::Fact, "b", "another one"));
    page(&store, &p, "sessions", "# sessions\n\npagesurvivortoken\n");
    reindex(&w);

    // A fresh machine whose hub has not materialised yet: the store simply is
    // not there. Emptying the index would look like data loss.
    std::fs::remove_dir_all(&store.root).unwrap();
    let index = open_read(&w);
    let outcome = index.reindex(&store, false).unwrap();
    assert!(outcome.delete_phase_skipped);
    assert_eq!(outcome.deleted, 0);
    assert_eq!(index.count_items().unwrap(), 2);
    assert_eq!(fts_hits(&index, "survivortoken"), 1);
    assert_eq!(index.count_pages().unwrap(), 1);
    assert_eq!(page_hits(&index, "pagesurvivortoken"), 1);
}

#[test]
fn archived_items_are_inactive_and_excluded_by_default() {
    let w = World::new("idx-archived");
    let p = w.project("01K2AAAAAAAAAAAAAAAAAAAAAA", "thing");
    let store = w.store();
    let mut archived = item(Kind::Fact, "old news", "archivedtoken");
    archived.meta.archived = Some(true);
    archived.meta.archived_at = Some("2026-08-01T00:00:00Z".parse().unwrap());
    put(&store, Some(&p), &archived);
    put(&store, Some(&p), &item(Kind::Fact, "current", "livetoken"));

    reindex(&w);
    let index = open_read(&w);
    assert_eq!(index.count_items().unwrap(), 2);
    assert_eq!(index.count_active().unwrap(), 1);
    let row = index.get(&archived.meta.id).unwrap().expect("archived row");
    assert!(row.archived);
    assert!(!row.active);
}

#[test]
fn superseding_marks_the_old_item_inactive_and_leaves_its_file_alone() {
    let w = World::new("idx-supersede");
    let p = w.project("01K2AAAAAAAAAAAAAAAAAAAAAA", "thing");
    let store = w.store();
    let old = item(Kind::Fact, "sessions in the database", "old body");
    let old_path = put(&store, Some(&p), &old);
    let old_bytes = std::fs::read(&old_path).unwrap();

    let mut new = item(Kind::Fact, "sessions in redis", "new body");
    new.meta.supersedes = Some(old.meta.id.clone());
    put(&store, Some(&p), &new);

    reindex(&w);
    let index = open_read(&w);
    let old_row = index.get(&old.meta.id).unwrap().unwrap();
    let new_row = index.get(&new.meta.id).unwrap().unwrap();
    assert!(!old_row.active);
    assert_eq!(old_row.superseded_by.as_deref(), Some(new.meta.id.as_str()));
    assert!(new_row.active);
    assert_eq!(
        std::fs::read(&old_path).unwrap(),
        old_bytes,
        "superseding must never modify the superseded file"
    );
}

#[test]
fn supersede_edges_are_indexed_and_a_late_supersede_still_flips_the_old_item() {
    let w = World::new("idx-supersede-late");
    let p = w.project("01K2AAAAAAAAAAAAAAAAAAAAAA", "thing");
    let store = w.store();
    let old = item(Kind::Fact, "sessions in the database", "old body");
    put(&store, Some(&p), &old);
    reindex(&w);

    // The correlated subquery behind `active` runs before every read, so the
    // edge column has to be indexed: without this it is a full scan per row.
    let index = open_read(&w);
    let indexes: Vec<String> = {
        let mut stmt = index
            .conn
            .prepare("SELECT name FROM sqlite_master WHERE type='index' AND tbl_name='items'")
            .unwrap();
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).unwrap();
        rows.filter_map(|r| r.ok()).collect()
    };
    assert!(
        indexes.iter().any(|n| n == "idx_items_supersedes"),
        "{indexes:?}"
    );

    // A pass that touched nothing skips the recompute, so what the last pass
    // derived has to still be standing.
    let again = index.reindex(&store, false).unwrap();
    assert_eq!(again.indexed, 0);
    assert!(index.get(&old.meta.id).unwrap().unwrap().active);

    // And a supersede that arrives later still flips the old item, which is
    // what makes the skip safe: a row landed, so the recompute ran.
    let mut new = item(Kind::Fact, "sessions in redis", "new body");
    new.meta.supersedes = Some(old.meta.id.clone());
    put(&store, Some(&p), &new);
    let outcome = index.reindex(&store, false).unwrap();
    assert_eq!(outcome.indexed, 1);
    assert!(!index.get(&old.meta.id).unwrap().unwrap().active);
}

#[test]
fn a_supersede_fork_leaves_both_replacements_active_and_is_reportable() {
    let w = World::new("idx-fork");
    let p = w.project("01K2AAAAAAAAAAAAAAAAAAAAAA", "thing");
    let store = w.store();
    let old = item(Kind::Fact, "original", "old body");
    put(&store, Some(&p), &old);
    for title in ["fork from a", "fork from b"] {
        let mut fork = item(Kind::Fact, title, "body");
        fork.meta.supersedes = Some(old.meta.id.clone());
        put(&store, Some(&p), &fork);
    }

    reindex(&w);
    let index = open_read(&w);
    assert_eq!(
        index.count_active().unwrap(),
        2,
        "both replacements stay active"
    );
    let forks = index.supersede_forks().unwrap();
    assert_eq!(forks.len(), 1);
    assert_eq!(forks[0].0, old.meta.id);
    assert_eq!(forks[0].1.len(), 2);
}

#[test]
fn temps_conflict_losers_and_strays_never_enter_the_index() {
    let w = World::new("idx-strays");
    let p = w.project("01K2AAAAAAAAAAAAAAAAAAAAAA", "thing");
    let store = w.store();
    put(&store, Some(&p), &item(Kind::Fact, "real", "realtoken"));
    let dir = store.project_items(&p);
    std::fs::write(
        dir.join(".tmp-99-01K2YR1VC0AB3DE4FG5HJ6KM7N.md"),
        b"+++\n+++\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("01K2YR1VC0AB3DE4FG5HJ6KM7P.md.path1"),
        b"conflict loser",
    )
    .unwrap();
    std::fs::write(dir.join("README.md"), b"not an item").unwrap();

    reindex(&w);
    let index = open_read(&w);
    assert_eq!(index.count_items().unwrap(), 1);
    assert_eq!(store.stray_paths().len(), 3);
}

#[test]
fn an_unparseable_item_is_skipped_rather_than_failing_the_reindex() {
    let w = World::new("idx-corrupt");
    let p = w.project("01K2AAAAAAAAAAAAAAAAAAAAAA", "thing");
    let store = w.store();
    put(&store, Some(&p), &item(Kind::Fact, "good", "goodtoken"));
    std::fs::write(
        store
            .project_items(&p)
            .join("01K2YR1VC0AB3DE4FG5HJ6KM7N.md"),
        b"this file has no frontmatter at all",
    )
    .unwrap();

    let outcome = reindex(&w);
    assert_eq!(outcome.indexed, 1);
    assert_eq!(outcome.unreadable, 1);
    assert_eq!(open_read(&w).count_items().unwrap(), 1);
}

#[test]
fn a_reindex_skips_immediately_when_another_one_holds_the_lock() {
    let w = World::new("idx-busy");
    let p = w.project("01K2AAAAAAAAAAAAAAAAAAAAAA", "thing");
    let store = w.store();
    put(&store, Some(&p), &item(Kind::Fact, "first", "firsttoken"));
    reindex(&w);

    // Another invocation is mid-reindex: it holds the write lock.
    let mut holder = open_read(&w);
    let held = holder.begin_immediate().expect("hold the write lock");

    put(&store, Some(&p), &item(Kind::Fact, "second", "secondtoken"));
    let reader = open_read(&w);
    let started = Instant::now();
    let outcome = reader
        .reindex(&store, false)
        .expect("a busy index is not an error");
    let elapsed = started.elapsed();

    assert!(outcome.skipped, "a held lock means skip, not wait");
    assert!(
        elapsed < Duration::from_secs(1),
        "the stale path must not stall on a busy handler: took {elapsed:?}"
    );
    // The reader still serves what the index already had.
    assert_eq!(reader.count_items().unwrap(), 1);
    assert_eq!(fts_hits(&reader, "firsttoken"), 1);
    drop(held);

    // Once the lock is free the next read catches up.
    let outcome = reader.reindex(&store, false).unwrap();
    assert!(!outcome.skipped);
    assert_eq!(reader.count_items().unwrap(), 2);
}

#[test]
fn a_full_rebuild_reindexes_everything_and_leaves_no_duplicates() {
    let w = World::new("idx-full");
    let p = w.project("01K2AAAAAAAAAAAAAAAAAAAAAA", "thing");
    let store = w.store();
    for n in 0..5 {
        put(
            &store,
            Some(&p),
            &item(Kind::Fact, &format!("t{n}"), "sharedtoken here"),
        );
    }
    reindex(&w);
    let index = open_read(&w);
    let outcome = index.reindex(&store, true).unwrap();
    assert_eq!(outcome.indexed, 5);
    assert_eq!(index.count_items().unwrap(), 5);
    assert_eq!(
        fts_hits(&index, "sharedtoken"),
        5,
        "a full rebuild must not double up the FTS side"
    );
}

#[test]
fn the_index_asserts_fts5_and_a_new_enough_sqlite() {
    let w = World::new("idx-assert");
    let index = open_read(&w);
    assert!(index.has_fts5().unwrap());
    let (major, minor, _) = index.sqlite_version().unwrap();
    assert!(
        (major, minor) >= (3, 43),
        "contentless_delete needs SQLite 3.43+, got {major}.{minor}"
    );
}

#[test]
fn a_wiki_page_is_indexed_beside_the_items_and_leaves_with_its_file() {
    let w = World::new("idx-wiki");
    let p = w.project("01K2AAAAAAAAAAAAAAAAAAAAAA", "thing");
    let store = w.store();
    put(&store, Some(&p), &item(Kind::Fact, "an item", "itemtoken"));
    let path = page(&store, &p, "sessions", "# How sessions work\n\npagetoken\n");

    let outcome = reindex(&w);
    assert_eq!(outcome.indexed, 2, "a page is indexed with the items");
    let index = open_read(&w);
    assert_eq!(index.count_items().unwrap(), 1, "a page is not an item");
    assert_eq!(index.count_pages().unwrap(), 1);
    assert_eq!(page_hits(&index, "pagetoken"), 1);
    assert_eq!(
        fts_hits(&index, "pagetoken"),
        0,
        "pages match on their own table, never as items"
    );

    // Nothing changed: no work, and the page side does not double up.
    let again = index.reindex(&store, false).unwrap();
    assert_eq!(again.indexed, 0);
    assert_eq!(page_hits(&index, "pagetoken"), 1);

    std::fs::remove_file(&path).unwrap();
    let outcome = index.reindex(&store, false).unwrap();
    assert_eq!(outcome.deleted, 1);
    assert_eq!(index.count_pages().unwrap(), 0);
    assert_eq!(
        page_hits(&index, "pagetoken"),
        0,
        "the FTS row must go with the page"
    );
    assert_eq!(index.count_items().unwrap(), 1, "the item is untouched");
}

#[test]
fn an_edited_page_is_reindexed_and_the_old_text_stops_matching() {
    let w = World::new("idx-wiki-edit");
    let p = w.project("01K2AAAAAAAAAAAAAAAAAAAAAA", "thing");
    let store = w.store();
    page(&store, &p, "sessions", "# Old heading\n\nalphapagetoken\n");
    reindex(&w);
    let index = open_read(&w);
    assert_eq!(page_hits(&index, "alphapagetoken"), 1);

    page(&store, &p, "sessions", "# New heading\n\nbetapagetoken\n");
    let outcome = index.reindex(&store, false).unwrap();
    assert_eq!(outcome.indexed, 1);
    assert_eq!(
        index.count_pages().unwrap(),
        1,
        "a rewrite is not a new page"
    );
    assert_eq!(page_hits(&index, "alphapagetoken"), 0);
    assert_eq!(page_hits(&index, "betapagetoken"), 1);
    assert_eq!(
        page_hits(&index, "\"New heading\""),
        1,
        "the heading is the title, and it is searchable"
    );
}

#[test]
fn a_full_rebuild_covers_pages_without_duplicating_them() {
    let w = World::new("idx-wiki-full");
    let p = w.project("01K2AAAAAAAAAAAAAAAAAAAAAA", "thing");
    let store = w.store();
    for n in 0..3 {
        page(
            &store,
            &p,
            &format!("page-{n}"),
            "# heading\n\nsharedpagetoken\n",
        );
    }
    reindex(&w);
    let index = open_read(&w);
    let outcome = index.reindex(&store, true).unwrap();
    assert_eq!(outcome.indexed, 3);
    assert_eq!(index.count_pages().unwrap(), 3);
    assert_eq!(
        page_hits(&index, "sharedpagetoken"),
        3,
        "a full rebuild must not double up the page side"
    );
}

#[test]
fn temps_conflict_losers_and_strays_never_enter_the_wiki_index() {
    let w = World::new("idx-wiki-strays");
    let p = w.project("01K2AAAAAAAAAAAAAAAAAAAAAA", "thing");
    let store = w.store();
    page(&store, &p, "sessions", "# real\n\nrealpagetoken\n");
    let dir = store.wiki_dir(&p);
    for name in [
        ".tmp-99-sessions.md",
        "sessions.md.path1",
        "Sessions.md",
        "-leading-dash.md",
        "notes.txt",
        "sessions",
    ] {
        std::fs::write(dir.join(name), b"# stray\n\nstraypagetoken\n").unwrap();
    }
    std::fs::create_dir_all(dir.join("nested.md")).unwrap();

    reindex(&w);
    let index = open_read(&w);
    assert_eq!(index.count_pages().unwrap(), 1);
    assert_eq!(page_hits(&index, "straypagetoken"), 0);
    assert_eq!(page_hits(&index, "realpagetoken"), 1);
}

#[test]
fn a_page_that_is_not_text_is_skipped_rather_than_failing_the_reindex() {
    let w = World::new("idx-wiki-binary");
    let p = w.project("01K2AAAAAAAAAAAAAAAAAAAAAA", "thing");
    let store = w.store();
    page(&store, &p, "good", "# good\n\ngoodpagetoken\n");
    std::fs::write(store.wiki_page(&p, "binary"), [0xff, 0xfe, 0x00]).unwrap();

    let outcome = reindex(&w);
    assert_eq!(outcome.indexed, 1);
    assert_eq!(outcome.unreadable, 1);
    let index = open_read(&w);
    assert_eq!(index.count_pages().unwrap(), 1);
    assert_eq!(page_hits(&index, "goodpagetoken"), 1);
}

#[test]
fn a_corrupt_index_file_is_rebuilt_rather_than_fatal() {
    let w = World::new("idx-rebuild");
    let p = w.project("01K2AAAAAAAAAAAAAAAAAAAAAA", "thing");
    let store = w.store();
    put(&store, Some(&p), &item(Kind::Fact, "a", "rebuilttoken"));
    reindex(&w);

    std::fs::write(w.index_path(), b"this is not a database").unwrap();
    let index = Index::open(&w.index_path(), Purpose::Read).expect("a corrupt index is rebuilt");
    index.reindex(&store, false).unwrap();
    assert_eq!(index.count_items().unwrap(), 1);
    assert_eq!(fts_hits(&index, "rebuilttoken"), 1);
}
