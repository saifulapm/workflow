//! Write integrity (spec §7 write path, AC4).

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use common::TempDir;
use mem::atomic::{Step, read_mtime, trace, write_atomic, write_atomic_cas};
use mem::item::{Item, Kind, Meta};

fn item(version: usize) -> Item {
    let meta = Meta::new(
        "01K2YR1VC0AB3DE4FG5HJ6KM7N".to_string(),
        Kind::Fact,
        format!("v{version}"),
        "macbook-m2".to_string(),
    );
    let body = format!("v{version}\n").repeat(400);
    Item::new(meta, body.into_bytes())
}

#[test]
fn write_creates_the_file_and_leaves_no_temp() {
    let dir = TempDir::new("atomic");
    let path = dir.join("a/b/item.md");
    write_atomic(&path, b"hello").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"hello");
    let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with(".tmp-"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "temp files left behind: {leftovers:?}"
    );
}

#[test]
fn write_replaces_an_existing_file() {
    let dir = TempDir::new("atomic-replace");
    let path = dir.join("item.md");
    write_atomic(&path, b"first").unwrap();
    write_atomic(&path, b"second").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"second");
}

#[test]
fn the_durability_sequence_is_same_dir_temp_fsync_rename_fsync_dir() {
    let dir = TempDir::new("atomic-trace");
    let path = dir.join("sub/item.md");
    let t = trace();
    write_atomic(&path, b"payload").unwrap();
    let steps = t.steps();
    assert_eq!(steps.len(), 4, "{steps:?}");
    match &steps[0] {
        Step::Temp(tmp) => {
            assert_eq!(
                tmp.parent().unwrap(),
                path.parent().unwrap(),
                "temp must be in the destination directory (rename must not cross filesystems)"
            );
            let name = tmp.file_name().unwrap().to_string_lossy().to_string();
            assert!(
                name.starts_with(".tmp-"),
                "temp must be dot-prefixed: {name}"
            );
        }
        other => panic!("first step was {other:?}"),
    }
    assert_eq!(steps[1], Step::SyncFile);
    match &steps[2] {
        Step::Rename { to, .. } => assert_eq!(to, &path),
        other => panic!("third step was {other:?}"),
    }
    assert_eq!(
        steps[3],
        Step::SyncDir(path.parent().unwrap().to_path_buf())
    );
}

#[test]
fn a_reader_never_sees_a_missing_torn_or_mixed_file() {
    let dir = TempDir::new("atomic-reader");
    let path = dir.join("item.md");
    write_atomic(&path, &item(0).to_bytes().unwrap()).unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let reads = Arc::new(AtomicUsize::new(0));
    let reader = {
        let path = path.clone();
        let stop = Arc::clone(&stop);
        let reads = Arc::clone(&reads);
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let bytes = std::fs::read(&path).expect("item file must never be missing");
                let parsed = Item::parse(&bytes).expect("frontmatter must never be torn");
                let marker = format!("{}\n", parsed.meta.title);
                let body = String::from_utf8(parsed.body).expect("body must never be torn");
                assert_eq!(
                    body,
                    marker.repeat(body.len() / marker.len()),
                    "content from two versions must never be mixed"
                );
                assert_eq!(body.len() % marker.len(), 0, "trailing partial line");
                reads.fetch_add(1, Ordering::Relaxed);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        })
    };

    for v in 1..=1000 {
        write_atomic(&path, &item(v).to_bytes().unwrap()).unwrap();
    }
    stop.store(true, Ordering::Relaxed);
    reader.join().expect("reader saw a broken file");
    assert!(reads.load(Ordering::Relaxed) > 0, "reader never ran");
}

#[test]
fn cas_refuses_to_clobber_a_changed_file() {
    let dir = TempDir::new("atomic-cas");
    let path = dir.join("status.md");
    write_atomic(&path, b"first").unwrap();
    let seen = read_mtime(&path);

    // Same mtime as observed: the write lands.
    assert!(write_atomic_cas(&path, b"second", seen).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), b"second");

    // Someone else wrote in between: refuse, and leave their content alone.
    std::thread::sleep(std::time::Duration::from_millis(20));
    write_atomic(&path, b"theirs").unwrap();
    assert!(!write_atomic_cas(&path, b"mine", seen).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), b"theirs");
}

#[test]
fn cas_on_a_file_that_appeared_since_the_read_refuses() {
    let dir = TempDir::new("atomic-cas-new");
    let path = dir.join("plan.md");
    assert_eq!(read_mtime(&path), None);
    write_atomic(&path, b"someone else got there first").unwrap();
    assert!(!write_atomic_cas(&path, b"mine", None).unwrap());
    assert_eq!(
        std::fs::read(&path).unwrap(),
        b"someone else got there first"
    );
}
