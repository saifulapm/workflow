//! Short-ID minting and store-wide uniqueness (spec §4, AC6).

mod common;

use std::collections::HashSet;

use common::TempDir;
use mem::ids::{IdRef, ShortIds, short_id};
use mem::item::{Item, Kind, Meta};
use mem::store::Store;

fn store(dir: &TempDir) -> Store {
    Store::new(dir.join("store"))
}

#[test]
fn a_scan_of_an_absent_store_is_empty() {
    let dir = TempDir::new("ids-absent");
    let ids = ShortIds::scan(&dir.join("nothing/here"));
    assert!(ids.is_empty());
}

#[test]
fn a_scan_sees_items_and_ignores_everything_else() {
    let dir = TempDir::new("ids-scan");
    let store = store(&dir);
    let items = store.global_items();
    std::fs::create_dir_all(&items).unwrap();
    std::fs::write(items.join("01K2YR1VC0AB3DE4FG5HJ6KM7N.md"), b"x").unwrap();
    std::fs::write(items.join("01K2YR1VC0AB3DE4FG5HJ6KM7P.md.path1"), b"x").unwrap();
    std::fs::write(items.join(".tmp-1-01K2YR1VC0AB3DE4FG5HJ6KM7Q.md"), b"x").unwrap();
    std::fs::write(items.join("notes.md"), b"x").unwrap();

    let ids = ShortIds::scan(&store.root);
    assert_eq!(ids.len(), 1);
    assert!(ids.contains("5HJ6KM7N"));
    assert!(!ids.contains("5HJ6KM7P"));
    assert_eq!(store.item_paths().len(), 1);
    assert_eq!(store.stray_paths().len(), 3);
}

#[test]
fn a_scan_covers_every_project_and_the_global_tree() {
    let dir = TempDir::new("ids-multi");
    let store = store(&dir);
    for d in [
        store.global_items(),
        store.project_items("01K2AAAAAAAAAAAAAAAAAAAAAA"),
        store.project_items("01K2BBBBBBBBBBBBBBBBBBBBBB"),
    ] {
        std::fs::create_dir_all(&d).unwrap();
    }
    std::fs::write(
        store.global_items().join("01K2YR1VC0AB3DE4FG5HJ6KM7N.md"),
        b"x",
    )
    .unwrap();
    std::fs::write(
        store
            .project_items("01K2AAAAAAAAAAAAAAAAAAAAAA")
            .join("01K2YR1VC0AB3DE4FG5HJ6KM7P.md"),
        b"x",
    )
    .unwrap();
    std::fs::write(
        store
            .project_items("01K2BBBBBBBBBBBBBBBBBBBBBB")
            .join("01K2YR1VC0AB3DE4FG5HJ6KM7Q.md"),
        b"x",
    )
    .unwrap();

    let ids = ShortIds::scan(&store.root);
    assert_eq!(ids.len(), 3, "uniqueness is store-wide, not per-project");
}

#[test]
fn minting_avoids_a_suffix_already_on_disk() {
    let dir = TempDir::new("ids-remint");
    let store = store(&dir);
    std::fs::create_dir_all(store.global_items()).unwrap();
    std::fs::write(
        store.global_items().join("01K2YR1VC0AB3DE4FG5HJ6KM7N.md"),
        b"x",
    )
    .unwrap();

    let mut ids = ShortIds::scan(&store.root);
    let taken = ulid::Ulid::from_string("01K2YR1VC0AB3DE4FG5HJ6KM7N").unwrap();
    let free = ulid::Ulid::from_string("01K2ZZ1VC0AB3DE4FG5HJ6KM7P").unwrap();
    let mut seq = [taken, free].into_iter();
    let minted = ids.mint_from(|| seq.next().unwrap());
    assert_eq!(minted, free, "a suffix already on disk must be re-minted");
}

#[test]
fn ten_thousand_items_in_a_tight_loop_have_no_suffix_collisions() {
    let dir = TempDir::new("ids-10k");
    let store = store(&dir);
    let items = store.global_items();
    std::fs::create_dir_all(&items).unwrap();

    let mut ids = ShortIds::scan(&store.root);
    let mut minted = HashSet::new();
    for n in 0..10_000 {
        let id = ids.mint().to_string();
        assert!(minted.insert(short_id(&id)), "minted a duplicate suffix");
        let meta = Meta::new(id, Kind::Log, format!("item {n}"), "macbook-m2".into());
        store
            .write_item(&items, &Item::new(meta, b"body\n".to_vec()))
            .unwrap();
    }

    // The filesystem is the authority, so check it rather than the in-memory set.
    let on_disk = store.item_paths();
    assert_eq!(on_disk.len(), 10_000);
    let suffixes: HashSet<String> = on_disk
        .iter()
        .map(|p| short_id(p.file_stem().unwrap().to_str().unwrap()))
        .collect();
    assert_eq!(suffixes.len(), 10_000, "suffix collision on disk");

    // A fresh scan of that store sees exactly the same suffixes.
    let rescan = ShortIds::scan(&store.root);
    assert_eq!(rescan.len(), 10_000);
    for s in &suffixes {
        assert!(rescan.contains(s));
    }
}

#[test]
fn lookups_take_full_ulids_and_exact_suffixes_only() {
    let full = "01K2YR1VC0AB3DE4FG5HJ6KM7N";
    assert_eq!(IdRef::parse(full), Some(IdRef::Full(full.to_string())));
    assert_eq!(
        IdRef::parse(&short_id(full)),
        Some(IdRef::Short("5HJ6KM7N".to_string()))
    );
    // Note `01K2YR1V` is a well-formed 8-char reference — it simply matches no
    // item. What must never parse is a prefix of some other length.
    for bad in ["01K2", &full[..25], &full[..9], "5HJ6KM", "not-an-id"] {
        assert_eq!(IdRef::parse(bad), None, "{bad} must not resolve");
    }
}
