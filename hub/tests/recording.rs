//! The recording `mem` the hub tests read argvs from: a record written by
//! one call is never split by another call writing at the same time.

mod common;

use std::process::Command;

use common::{TempDir, invocations, recording_mem};

#[test]
fn concurrent_calls_leave_whole_records() {
    if common::real_mem().is_none() {
        panic!("build mem: cargo build --release --manifest-path ../mem/Cargo.toml");
    }
    let dir = TempDir::new("recording-whole");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let (bin, log) = recording_mem(dir.path(), &home);
    let args: Vec<String> = (0..40).map(|n| format!("arg{n}")).collect();
    let children: Vec<_> = (0..12)
        .map(|_| {
            Command::new(bin.join("mem"))
                .arg("--version")
                .args(&args)
                .env("PATH", "/usr/bin:/bin")
                .spawn()
                .unwrap()
        })
        .collect();
    for mut child in children {
        child.wait().unwrap();
    }
    let records = invocations(&log);
    assert_eq!(records.len(), 12, "{records:?}");
    for record in records {
        assert_eq!(record.len(), 41, "{record:?}");
        assert_eq!(&record[1..], &args[..], "{record:?}");
    }
}
