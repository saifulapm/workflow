//! Is anyone watching this machine? (the doorbell's routing question)
//!
//! The shell keeps its session state as flag files under
//! `$XDG_STATE_HOME/qshell/` — `locked` while the session is locked,
//! `stay-awake` while the bar toggle holds idle off, and `idle` while the
//! idle cycle is active (Services/Idle.qml, all in the file-is-the-state
//! pattern). Presence reads those three files plus one process check.
//!
//! Positive evidence only: a machine with no qshell state directory has never
//! run a shell (the nuc), and a state directory whose shell process is gone is
//! a dead session that may have left a stale `locked` marker behind. Neither
//! is watching. The failure mode this ordering buys: a wrongly-silent phone is
//! worse than a wrongly-noisy one, so anything unreadable reads as "not
//! watching" and the phone rings.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

/// The shell's process name — `/usr/bin/qs` on these machines.
const SHELL_PROCESS: &str = "qs";

/// How long a route device may be locked or idle before its questions go to
/// Phone.
pub const AWAY_AFTER: Duration = Duration::from_secs(5 * 60);

pub struct Presence {
    pub shell: bool,
    pub locked: bool,
    pub stay_awake: bool,
    pub idle: bool,
    /// How long `locked` has been set, from its file's age.
    pub locked_for: Duration,
    /// How long the idle cycle has run, from its file's age.
    pub idle_for: Duration,
}

impl Presence {
    /// Watching means: a live shell, unlocked, and either stay-awake is on
    /// (parked at the machine on purpose — the most-watching state there is)
    /// or the idle cycle has not started.
    pub fn watching(&self) -> bool {
        self.shell && !self.locked && (self.stay_awake || !self.idle)
    }

    /// Whether a question ringing here would go unseen: no shell, or locked
    /// or idle (stay-awake aside) for `AWAY_AFTER`. A lock or an idle spell
    /// shorter than that still takes the question; Saiful is likely near.
    pub fn away(&self) -> bool {
        !self.shell
            || (self.locked && self.locked_for >= AWAY_AFTER)
            || (self.idle && !self.stay_awake && self.idle_for >= AWAY_AFTER)
    }

    fn absent() -> Presence {
        Presence {
            shell: false,
            locked: false,
            stay_awake: false,
            idle: false,
            locked_for: Duration::ZERO,
            idle_for: Duration::ZERO,
        }
    }
}

pub fn sample() -> Presence {
    let Ok(state) = crate::config::state_home() else {
        return Presence::absent();
    };
    let dir = state.join("qshell");
    if !dir.is_dir() {
        return Presence::absent();
    }
    let locked = dir.join("locked");
    let idle = dir.join("idle");
    Presence {
        shell: shell_alive(),
        locked: locked.is_file(),
        stay_awake: dir.join("stay-awake").is_file(),
        idle: idle.is_file(),
        locked_for: age(&locked),
        idle_for: age(&idle),
    }
}

/// How long ago `path` was last written, zero when it is missing or newer
/// than the clock.
fn age(path: &Path) -> Duration {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|at| SystemTime::now().duration_since(at).ok())
        .unwrap_or_default()
}

/// `pgrep -x qs` — by argv, like every process this service runs. An exact
/// name: `-f` would match this service's own command line the day it carries
/// the word, which is the self-match trap the reap code already met once.
fn shell_alive() -> bool {
    Command::new("pgrep")
        .args(["-x", SHELL_PROCESS])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn presence(shell: bool, locked: bool, stay_awake: bool, idle: bool) -> Presence {
        Presence {
            shell,
            locked,
            stay_awake,
            idle,
            locked_for: Duration::ZERO,
            idle_for: Duration::ZERO,
        }
    }

    #[test]
    fn the_watching_truth_table() {
        // No shell is never watching, whatever the flags claim.
        assert!(!presence(false, false, false, false).watching());
        assert!(!presence(false, false, true, false).watching());
        // A live unlocked shell is watching until the idle cycle starts.
        assert!(presence(true, false, false, false).watching());
        assert!(!presence(true, false, false, true).watching());
        // Stay-awake overrides idle; a lock overrides everything.
        assert!(presence(true, false, true, true).watching());
        assert!(!presence(true, true, true, false).watching());
    }

    #[test]
    fn away_is_no_shell_or_five_minutes_locked_or_idle() {
        let five = AWAY_AFTER;
        let just = Duration::from_secs(60);
        assert!(presence(false, false, false, false).away());
        assert!(!presence(true, false, false, false).away());
        let mut locked = presence(true, true, false, false);
        locked.locked_for = just;
        assert!(!locked.away());
        locked.locked_for = five;
        assert!(locked.away());
        let mut idle = presence(true, false, false, true);
        idle.idle_for = just;
        assert!(!idle.away());
        idle.idle_for = five;
        assert!(idle.away());
        // Stay-awake holds idle off, but never a lock.
        idle.stay_awake = true;
        assert!(!idle.away());
        locked.stay_awake = true;
        assert!(locked.away());
    }
}
