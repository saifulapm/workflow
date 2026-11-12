//! A scratch directory for tests, removed when it goes out of scope. The
//! integration tests include this file by path, so each helper is used by some
//! test or other and not by all.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

pub struct Scratch(PathBuf);

impl Scratch {
    pub fn new(tag: &str) -> Scratch {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "workflow-test-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn at(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }

    pub fn mkdir(&self, rel: &str) -> PathBuf {
        let path = self.at(rel);
        fs::create_dir_all(&path).unwrap();
        path
    }

    pub fn put(&self, rel: &str, text: &str) {
        let path = self.at(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    /// A shell script, executable. A child process writes it: a sibling
    /// test's child forked while this process held it open for writing would
    /// keep that handle until its exec, and running the script would then
    /// fail with "Text file busy".
    pub fn script(&self, rel: &str, body: &str) -> PathBuf {
        let path = self.at(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let written = Command::new("sh")
            .args(["-c", r#"printf '%s\n' "$2" >"$1" && chmod 755 "$1""#, "sh"])
            .arg(&path)
            .arg(format!("#!/bin/sh\n{body}"))
            .status()
            .unwrap();
        assert!(written.success(), "could not write {}", path.display());
        path
    }

    pub fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.at(rel)).unwrap()
    }

    /// Is anything there, a dangling link included?
    pub fn has(&self, rel: &str) -> bool {
        fs::symlink_metadata(self.at(rel)).is_ok()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
