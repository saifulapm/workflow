//! The qshell-sync seam (spec §11). mem never bisyncs anything itself: it asks
//! qshell-sync to run the `memory` unit and reads the status file the sync
//! script and the shell panel already share.

use std::path::Path;
use std::time::Duration;

use jiff::Timestamp;
use serde::Deserialize;

/// The unit in qshell-sync that carries the mem store.
pub const UNIT: &str = "memory";

/// Older than this and `mem context` says so (spec §7).
pub const STALE_AFTER: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone, Deserialize, Default)]
pub struct UnitStatus {
    pub id: String,
    #[serde(default)]
    pub ok: Option<bool>,
    #[serde(default)]
    pub last_run: Option<String>,
    #[serde(default)]
    pub last_ok: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub error_kind: Option<String>,
}

/// `status.json` as qshell-sync writes it after every unit.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Status {
    #[serde(default)]
    pub machine: String,
    #[serde(default, rename = "lastRun")]
    pub last_run: String,
    #[serde(default)]
    pub running: bool,
    #[serde(default, rename = "currentUnit")]
    pub current_unit: String,
    #[serde(default)]
    pub units: Vec<RawUnit>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct RawUnit {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub ok: Option<bool>,
    #[serde(default, rename = "lastRun")]
    pub last_run: String,
    #[serde(default, rename = "lastOk")]
    pub last_ok: String,
    #[serde(default)]
    pub error: String,
    #[serde(default, rename = "errorKind")]
    pub error_kind: String,
}

impl Status {
    /// A missing or unparseable status file is "nothing to report" rather than
    /// an error: a machine that has never synced is a normal machine.
    pub fn read(path: &Path) -> Option<Status> {
        let text = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn unit(&self, id: &str) -> Option<&RawUnit> {
        self.units.iter().find(|u| u.id == id)
    }

    /// The one line `mem context` prefixes when the memory unit is unhealthy.
    pub fn staleness_warning(&self, now: Timestamp) -> Option<String> {
        let unit = self.unit(UNIT)?;
        if unit.ok == Some(false) {
            let detail = if unit.error.is_empty() {
                "sync is failing".to_string()
            } else {
                format!("sync is failing: {}", unit.error)
            };
            return Some(format!("! {detail}"));
        }
        let last_ok = unit.last_ok.parse::<Timestamp>().ok()?;
        let age = now.as_second() - last_ok.as_second();
        if age > STALE_AFTER.as_secs() as i64 {
            let minutes = age / 60;
            return Some(format!(
                "! memory last synced {minutes} min ago — another machine may be ahead"
            ));
        }
        None
    }
}

/// Warning line for a store whose sync status cannot be read at all: the unit
/// may simply not be wired up yet, which is not worth a line in every digest.
pub fn staleness_line(status_path: &Path, now: Timestamp) -> Option<String> {
    Status::read(status_path)?.staleness_warning(now)
}

/// How mem invokes the sync unit. Overridable so a test — or a machine where
/// qshell-sync is not installed — can exercise the path without a real bisync.
pub fn sync_command() -> String {
    std::env::var("MEM_SYNC_CMD").unwrap_or_else(|_| "qshell-sync".to_string())
}

/// Asks for a sync round and returns without waiting for it (spec §7, sync
/// semantics ruling). Handoff, ask, answer and the park path all want the other
/// machine to see them soon, and none of them can afford to sit through a
/// Dropbox round inside a runtime tool call that dies at 600 s: the CLI's
/// latency contract wins, and the 15-minute timer is the backstop. Verification
/// — comparing `lastRun` before and after — belongs to `mem sync` alone.
///
/// A round already in progress is left alone: qshell-sync holds a flock, so a
/// second request during one buys nothing.
pub fn trigger(status_path: &Path) {
    if Status::read(status_path).is_some_and(|s| s.running) {
        return;
    }
    let cmd = sync_command();
    let mut parts = cmd.split_whitespace();
    let Some(program) = parts.next() else { return };
    let args: Vec<&str> = parts.collect();
    // Spawned and never waited on: the child outlives this process and init
    // reaps it. Its handles go nowhere, so it can neither block on a pipe nor
    // land in the middle of mem's own output.
    let _ = std::process::Command::new(program)
        .args(args)
        .arg("--only")
        .arg(UNIT)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// What one `mem sync` did. qshell-sync exits 0 when its flock is held, so a
/// zero exit proves nothing: the verdict comes from comparing the status file
/// before and after.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Performed { detail: String },
    NotPerformed { reason: String },
    Deferred { reason: String },
}

fn run_once() -> std::io::Result<std::process::Output> {
    let cmd = sync_command();
    let mut parts = cmd.split_whitespace();
    let program = parts.next().unwrap_or("qshell-sync");
    let args: Vec<&str> = parts.collect();
    std::process::Command::new(program)
        .args(args)
        .arg("--only")
        .arg(UNIT)
        .output()
}

/// The verified sync of spec §7: compare `lastRun`/`running` before and after,
/// retry once after a pause, and say plainly when the round did not happen.
pub fn verified(status_path: &Path, pause: std::time::Duration) -> anyhow::Result<Outcome> {
    for attempt in 0..2 {
        let before = Status::read(status_path);
        if before.as_ref().is_some_and(|s| s.running) {
            if attempt == 0 {
                std::thread::sleep(pause);
                continue;
            }
            return Ok(Outcome::NotPerformed {
                reason: "another round is in progress; not performed".to_string(),
            });
        }
        let out = run_once().map_err(|e| {
            crate::exit::store_error(format!("could not run {}: {e}", sync_command()))
        })?;
        if !out.status.success() {
            // qshell-sync ran; the round failed. That is a verdict, not an
            // inability to run — exit 3 is for a qshell-sync this machine does
            // not have, which is a different thing to tell the caller.
            let detail = String::from_utf8_lossy(&out.stderr).trim().to_string();
            let detail = if detail.is_empty() {
                match out.status.code() {
                    Some(code) => format!("it exited {code}"),
                    None => "it was killed by a signal".to_string(),
                }
            } else {
                detail
            };
            return Ok(Outcome::NotPerformed {
                reason: format!("{} reported a failure: {detail}", sync_command()),
            });
        }
        let after = Status::read(status_path);
        let moved = match (&before, &after) {
            (Some(b), Some(a)) => a.last_run != b.last_run,
            (None, Some(_)) => true,
            _ => false,
        };
        if moved {
            let detail = after
                .and_then(|s| s.unit(UNIT).map(|u| u.last_ok.clone()))
                .unwrap_or_default();
            return Ok(Outcome::Performed { detail });
        }
        if attempt == 0 {
            std::thread::sleep(pause);
            continue;
        }
        return Ok(Outcome::Deferred {
            reason: "the round did not run; the 15-minute timer will pick it up".to_string(),
        });
    }
    Ok(Outcome::Deferred {
        reason: "the round did not run; the 15-minute timer will pick it up".to_string(),
    })
}
