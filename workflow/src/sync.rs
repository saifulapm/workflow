//! The qshell-sync seam for parked bundles (friction #KAPRWGBB). workflow
//! never bisyncs anything itself: it asks qshell-sync to run the `parked`
//! unit, the same way mem asks for the `memory` unit. qshell-sync holds its
//! own flock, so a request during a round buys nothing and breaks nothing.

use std::process::{Command, Stdio};

/// The unit in qshell-sync that carries the parked bundles.
pub const UNIT: &str = "parked";

/// Overridable so a test -- or a machine without qshell-sync -- can exercise
/// the path without a real bisync.
fn sync_command() -> String {
    std::env::var("WORKFLOW_SYNC_CMD").unwrap_or_else(|_| "qshell-sync".to_string())
}

fn command() -> Option<Command> {
    let cmd = sync_command();
    let mut parts = cmd.split_whitespace();
    let mut c = Command::new(parts.next()?);
    c.args(parts.collect::<Vec<&str>>())
        .arg("--only")
        .arg(UNIT)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    Some(c)
}

/// Ask for a round and return without waiting: park wants the other machine
/// to see the bundle soon and cannot afford to sit through a Dropbox round.
/// Spawned and never waited on -- init reaps the child, and its handles go
/// nowhere, so it can neither block on a pipe nor land in park's own output.
pub fn trigger() {
    if let Some(mut c) = command() {
        let _ = c.spawn();
    }
}

/// Run a round and wait it out: resume, missing its bundle, has nothing to do
/// until the round lands, so it can afford to.
pub fn await_round() -> bool {
    match command() {
        Some(mut c) => c.status().map(|s| s.success()).unwrap_or(false),
        None => false,
    }
}
