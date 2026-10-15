//! Bakes the commit the hub is built from into `HUB_COMMIT`, which
//! `hub --version` prints, so the installed hub can be checked against the
//! checkout.

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    out.status.success().then(|| text.trim().to_string())
}

fn main() {
    // A commit moves HEAD's branch ref, a checkout moves HEAD, and a gc packs
    // the ref away, so each of the three reruns this script. In a linked
    // worktree HEAD lives in its own git dir and the refs in the common one.
    let mut watched = vec!["HEAD".to_string(), "packed-refs".to_string()];
    if let Some(branch) = git(&["symbolic-ref", "-q", "HEAD"]) {
        watched.push(branch);
    }
    for path in &watched {
        if let Some(path) = git(&["rev-parse", "--path-format=absolute", "--git-path", path]) {
            println!("cargo::rerun-if-changed={path}");
        }
    }
    let commit = git(&["rev-parse", "--short=7", "HEAD"]).unwrap_or_else(|| "unknown".to_string());
    println!("cargo::rustc-env=HUB_COMMIT={commit}");
}
