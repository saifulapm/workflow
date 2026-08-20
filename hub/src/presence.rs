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

use std::process::{Command, Stdio};

/// The shell's process name — `/usr/bin/qs` on these machines.
const SHELL_PROCESS: &str = "qs";

pub struct Presence {
    pub shell: bool,
    pub locked: bool,
    pub stay_awake: bool,
    pub idle: bool,
}

impl Presence {
    /// Watching means: a live shell, unlocked, and either stay-awake is on
    /// (parked at the machine on purpose — the most-watching state there is)
    /// or the idle cycle has not started.
    pub fn watching(&self) -> bool {
        self.shell && !self.locked && (self.stay_awake || !self.idle)
    }

    fn absent() -> Presence {
        Presence {
            shell: false,
            locked: false,
            stay_awake: false,
            idle: false,
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
    Presence {
        shell: shell_alive(),
        locked: dir.join("locked").is_file(),
        stay_awake: dir.join("stay-awake").is_file(),
        idle: dir.join("idle").is_file(),
    }
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
}
