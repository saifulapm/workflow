//! The doorbell (spec §5).
//!
//! A background thread polls the pending queue every 15 s and rings once for
//! every question id it has not seen. What it publishes is deliberately thin:
//! the machine, the project, and a link back here — **never the question
//! text**.
//!
//! ### What the topic actually protects
//!
//! ntfy.sh is a public broker. Anyone holding the topic can read every message
//! published to it, and can publish to it. So what this discloses to whoever
//! holds the topic is the machine name, the project name, the hub URL, and the
//! timing of every question — not the question text. The topic is the only
//! secret: 128 bits, stored 0600, and never printed anywhere that is not
//! already behind the tailnet.
//!
//! ### Two rules about the seen file
//!
//! It is written and fsynced **before** the ring, so a crash between the two
//! loses a doorbell rather than looping on one — a missed buzz costs a delay,
//! a repeated one costs trust in the doorbell. And it survives a restart, which
//! is what makes `systemctl --user restart` silent (AC5).

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};

use crate::app::App;
use crate::memcli::MemCli;

pub const DEFAULT_POLL: Duration = Duration::from_secs(15);
/// §5's own command line: `curl -fsS -m 10 -d <body> <ntfy_base>/<topic>`.
pub const CURL_TIMEOUT_SECONDS: &str = "10";

pub struct Doorbell {
    pub seen_path: PathBuf,
    pub ntfy_url: String,
    pub machine: String,
    /// Where the buzz sends the phone.
    pub hub_url: String,
    pub poll: Duration,
}

impl Doorbell {
    pub fn from_app(app: &App) -> Result<Doorbell> {
        Ok(Doorbell {
            seen_path: crate::config::state_home()?.join("hub/seen"),
            ntfy_url: app.config.ntfy_url(),
            machine: app.machine.clone(),
            hub_url: hub_url(app),
            poll: poll_interval(),
        })
    }

    /// Runs until the process ends. Nothing in here may panic or return: the
    /// doorbell failing is not the service failing.
    pub fn run(self, mem: Arc<MemCli>) {
        let loaded = load(&self.seen_path);
        // No file at all means this is the first start on this machine. The
        // backlog is recorded and rung for zero times: a fresh install should
        // not buzz once per already-pending question, and neither should a
        // machine that has just joined and synced somebody else's queue.
        let mut state = State {
            seeding: loaded.is_none(),
            seen: loaded.unwrap_or_default(),
            record_failed: false,
        };
        loop {
            self.round(&mem, &mut state);
            std::thread::sleep(self.poll);
        }
    }

    fn round(&self, mem: &MemCli, state: &mut State) {
        // Fresh, not cached: a stale queue here is a late doorbell, and the
        // poll interval is already the rate limit.
        let outcome = mem.questions_fresh();
        // Silence is not an empty queue (review M-1). This used to ask
        // `outcome.broken()`, which is `None` for `Outcome::Absent`, so §4a's
        // "exit non-zero, empty stdout" row fell through, the loop over no rows
        // did nothing, and `seeding` was cleared anyway — a first start that
        // met one such poll then rang for the entire backlog. `list_fault` is
        // the page's own reading of §4a (decision 6.7), shared here so the two
        // readings of silence cannot drift apart again.
        if let Some(why) = crate::model::list_fault(&outcome, "questions") {
            // Not fatal, and not a reason to seed: try again next round.
            eprintln!("hub: doorbell: {why}");
            return;
        }
        for row in outcome.rows("questions") {
            let Some(id) = row["id"].as_str() else {
                continue;
            };
            if state.seen.contains(id) {
                continue;
            }
            // Recorded and fsynced first. If it cannot be recorded, it is not
            // rung either — otherwise a read-only state directory turns into a
            // buzz every fifteen seconds, for ever.
            if let Err(e) = remember(&self.seen_path, id) {
                // Once per run of failures, not once per poll for ever: an
                // unwritable seen file used to log four lines a minute, per
                // stuck question, indefinitely (review m-5).
                if !state.record_failed {
                    eprintln!("hub: doorbell: could not record {id}: {e:#}");
                    state.record_failed = true;
                }
                continue;
            }
            state.record_failed = false;
            state.seen.insert(id.to_string());
            if state.seeding {
                continue;
            }
            // A question born on another machine arrives here by sync, already
            // rung for where it was asked; this hub lists and answers it but
            // does not buzz again. A row with no machine field still rings —
            // a mem too old to say is a doorbell, not a silence.
            if row["machine"].as_str().is_some_and(|m| m != self.machine) {
                continue;
            }
            self.deliver(row["project"].as_str().unwrap_or(""));
        }
        state.seeding = false;
    }

    /// Where the bell rings: the phone, and only for an empty house. A watched
    /// machine gets nothing — whoever is at it sees questions in the session
    /// itself (a background agent asks through its own question tool when the
    /// machine is watched; `mem ask` is the locked-screen channel), so any
    /// bell here would be a duplicate on a screen already showing the thing.
    fn deliver(&self, project: &str) {
        if crate::presence::sample().watching() {
            return;
        }
        self.ring(&self.body(project));
    }

    /// One POST, by argv. Never retried: §5 says a failure is logged and
    /// forgotten, and the seen file has already moved on.
    fn ring(&self, body: &str) {
        let result = Command::new("curl")
            .args([
                "-fsS",
                "-m",
                CURL_TIMEOUT_SECONDS,
                "-d",
                body,
                &self.ntfy_url,
            ])
            .output();
        match result {
            Ok(out) if out.status.success() => {}
            Ok(out) => eprintln!(
                "hub: doorbell: ntfy said no ({}): {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            ),
            Err(e) => eprintln!("hub: doorbell: could not run curl: {e}"),
        }
    }

    /// The whole message. A project name and a URL, and no question text — the
    /// text is what the tailnet is for.
    pub fn body(&self, project: &str) -> String {
        if project.is_empty() {
            format!("question waiting on {} — {}", self.machine, self.hub_url)
        } else {
            format!(
                "question waiting on {} ({}) — {}",
                self.machine, project, self.hub_url
            )
        }
    }
}

/// What one run of the doorbell carries between rounds.
struct State {
    seen: BTreeSet<String>,
    /// True until one round has completed without a fault: the backlog is
    /// recorded, and rung for zero times.
    seeding: bool,
    /// Whether the last attempt to record an id failed, so the log says so once
    /// rather than every fifteen seconds (review m-5).
    record_failed: bool,
}

/// Where the buzz sends the phone: whatever the config names, else this
/// machine's tailnet name, else the machine name.
///
/// The machine name is a fallback and not the default any more, because it is
/// the one that was wrong. It comes from `~/.config/qshell/machine`, which here
/// reads `macbook-m2` — a name that resolves on no network at all, so the phone
/// opened a dead link. `Guard::host_allowed` already admits anything under
/// `.ts.net`, so the tailnet name passes hub's own Host check unchanged.
fn hub_url(app: &App) -> String {
    if let Some(url) = &app.config.hub_url {
        return url.clone();
    }
    let host = crate::tailnet_name().unwrap_or_else(|| app.machine.clone());
    format!("http://{host}:{}/", app.port)
}

/// 15 s, or `HUB_POLL_MS` — the same kind of seam as mem's `MEM_POLL_MS`, so a
/// test does not have to wait a quarter of a minute per round.
pub fn poll_interval() -> Duration {
    std::env::var("HUB_POLL_MS")
        .ok()
        .and_then(|ms| ms.parse().ok())
        .map(Duration::from_millis)
        // A zero from the seam would be a busy loop spawning a `mem` per pass.
        .map(|poll| poll.max(Duration::from_millis(10)))
        .unwrap_or(DEFAULT_POLL)
}

/// The ids already rung for, or `None` when the file has never existed.
pub fn load(path: &Path) -> Option<BTreeSet<String>> {
    let text = std::fs::read_to_string(path).ok()?;
    Some(
        text.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

/// Appends one id and fsyncs it, so the ring that follows can never be the
/// thing that survives a crash while the record of it does not.
pub fn remember(path: &Path, id: &str) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
        // The directory entry needs to be durable too, or the file can vanish
        // with the crash it was written to survive.
        if let Ok(dir) = std::fs::File::open(parent) {
            let _ = dir.sync_all();
        }
    }
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    writeln!(file, "{id}")?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_body_names_the_machine_and_project_and_nothing_else() {
        let doorbell = Doorbell {
            seen_path: PathBuf::from("/nonexistent"),
            ntfy_url: "http://127.0.0.1:9/workflow-TOPIC".to_string(),
            machine: "macbook".to_string(),
            hub_url: "http://macbook:8787/".to_string(),
            poll: DEFAULT_POLL,
        };
        assert_eq!(
            doorbell.body("proj-alpha"),
            "question waiting on macbook (proj-alpha) — http://macbook:8787/"
        );
        // A global question has no project, and the sentence still reads.
        assert_eq!(
            doorbell.body(""),
            "question waiting on macbook — http://macbook:8787/"
        );
    }
}
