//! The pre-commit rustfmt check. A commit that lands unformatted Rust turns a
//! crate's `cargo fmt --check` red for whoever builds next, so the hook runs
//! rustfmt over what is staged.
//!
//! It reads the staged blob (`git show :<path>`), not the work tree, because
//! the commit is made of the index. It runs from the crate's folder so a
//! `rustfmt.toml` there applies, and it fails open: no rustfmt on PATH means
//! no check.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::gitcmd::Git;
use crate::{exit, warn};

/// Check every staged `.rs` file. Prints a line for each one that rustfmt
/// would change and returns `exit::FAILED`, or `exit::OK`.
pub fn check_staged() -> i32 {
    let git = Git::here();
    let Some(top) = git.toplevel() else {
        return exit::OK;
    };
    let names = git.bytes(&[
        "diff",
        "--cached",
        "--name-only",
        "--diff-filter=ACMR",
        "-z",
    ]);
    let mut failed = false;
    for name in names.split(|b| *b == 0).filter(|n| !n.is_empty()) {
        let Ok(rel) = std::str::from_utf8(name) else {
            continue;
        };
        if !rel.ends_with(".rs") {
            continue;
        }
        let blob = git.bytes(&["show", &format!(":{rel}")]);
        let (dir, edition) = crate_of(&top, rel);
        if unformatted(&blob, &dir, &edition) == Some(true) {
            warn(format!(
                "hook: {rel} is not rustfmt-clean; run cargo fmt --manifest-path {}/Cargo.toml",
                dir.display()
            ));
            failed = true;
        }
    }
    if failed { exit::FAILED } else { exit::OK }
}

/// The folder of the nearest Cargo.toml above `rel` (the repo top when there is
/// none) and the edition it names, 2021 when it names none.
fn crate_of(top: &Path, rel: &str) -> (PathBuf, String) {
    let mut dir = top.join(rel);
    while dir.pop() {
        if let Ok(text) = std::fs::read_to_string(dir.join("Cargo.toml")) {
            return (dir, edition_in(&text).unwrap_or_else(|| "2021".into()));
        }
        if dir == top {
            break;
        }
    }
    (top.to_path_buf(), "2021".into())
}

/// The value of an `edition = "..."` line.
fn edition_in(manifest: &str) -> Option<String> {
    manifest.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("edition")?.trim_start();
        let value = rest.strip_prefix('=')?.trim();
        Some(value.trim_matches(['"', '\'']).to_string())
    })
}

/// Would rustfmt change `source`? `None` when rustfmt cannot be run or cannot
/// format it (a syntax error is the compiler's to report). It compares the
/// formatted text with the input because `rustfmt --check` on stdin exits 0
/// even when it prints a diff.
fn unformatted(source: &[u8], dir: &Path, edition: &str) -> Option<bool> {
    let mut child = Command::new("rustfmt")
        .args(["--edition", edition])
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdin = child.stdin.take()?;
    let input = source.to_vec();
    // A thread writes while this one reads, so a large file cannot fill both pipes.
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let out = child.wait_with_output().ok()?;
    let _ = writer.join();
    out.status.success().then(|| out.stdout != source)
}
