//! `workflow pause` and `workflow unpause` -- record in mem that a project is
//! paused, so `workflow go` starts nothing new for it, and clear that again.
//! Neither starts or stops an agent.

use crate::{exit, machine, memcli, warn};

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
    let who = by.map_or_else(machine::mem_name, str::to_string);
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
