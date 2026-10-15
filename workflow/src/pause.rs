//! `workflow pause` and `workflow unpause` -- record in mem that a project is
//! paused, so `workflow go` starts nothing new for it, and clear that again.
//! Neither starts or stops an agent.

use std::path::PathBuf;
use std::process::Command;

use crate::{exit, memcli, paths, warn};

/// This machine's name, read the way mem reads it: `qshell/machine` under
/// `$XDG_CONFIG_HOME` (else `~/.config`), else `uname -n`.
fn machine_name() -> String {
    let config = match std::env::var("XDG_CONFIG_HOME") {
        Ok(v) if !v.is_empty() => Some(PathBuf::from(v)),
        _ => paths::home().map(|home| home.join(".config")),
    };
    if let Some(text) = config.and_then(|c| std::fs::read_to_string(c.join("qshell/machine")).ok())
    {
        let name = text.trim();
        if !name.is_empty() {
            return name.to_string();
        }
    }
    if let Ok(out) = Command::new("uname").arg("-n").output()
        && out.status.success()
    {
        let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !name.is_empty() {
            return name;
        }
    }
    "unknown".to_string()
}

/// Refuse a name mem does not know, as `go` does: exit 2, or 1 when mem
/// cannot list its projects.
fn known(project: &str, verb: &str) -> Result<(), i32> {
    let Some(projects) = memcli::projects() else {
        warn(format!("{verb}: mem cannot list its projects"));
        return Err(exit::FAILED);
    };
    if projects.iter().any(|row| row.name == project) {
        return Ok(());
    }
    warn(format!(
        "{verb}: no project named {project}; `mem projects` lists them"
    ));
    Err(exit::USAGE)
}

pub fn cmd_pause(project: &str, by: Option<&str>) -> i32 {
    if let Err(code) = known(project, "pause") {
        return code;
    }
    let who = by.map_or_else(machine_name, str::to_string);
    if !memcli::set_paused(project, &who) {
        warn(format!(
            "pause: mem could not record the pause of {project}"
        ));
        return exit::FAILED;
    }
    println!("{project}: paused");
    exit::OK
}

pub fn cmd_unpause(project: &str) -> i32 {
    if let Err(code) = known(project, "unpause") {
        return code;
    }
    if !memcli::unset_paused(project) {
        warn(format!(
            "unpause: mem could not clear the pause of {project}"
        ));
        return exit::FAILED;
    }
    println!("{project}: unpaused");
    exit::OK
}
