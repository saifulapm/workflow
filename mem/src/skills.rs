//! The skill mem owns, and the section of the digest that names every skill a
//! session can open.
//!
//! A skill used to be a file `workflow doctor --fix` wrote into each harness's
//! skills directory, where the harness found it for itself. That is why turning
//! one off took a settings file per harness, and why a copy on disk could drift
//! from the binary. Skills are served now — `mem skill mem`, `workflow skill
//! <name>` — and named here, in the digest mem already injects at the start of
//! a session. Registration is the whole gate: [`crate::verbs::context`] says
//! nothing outside a project mem knows, so neither does this.

use std::process::{Command, Stdio};

use crate::exit;

/// The skill mem owns, embedded so the binary carries it wherever it runs.
///
/// One skill, because a skill belongs to the binary it is about: workflow
/// serves the other seven from `workflow/src/skill.rs`, and a machine with mem
/// and no workflow still has the skill that explains mem.
pub const SKILLS: [(&str, &str); 1] = [("mem", include_str!("../../skills/mem/SKILL.md"))];

/// What the section tells a model to do with the names under it. It has to name
/// both verbs: mem serves its own skill and workflow serves the rest, and a
/// session that guesses wrong gets an exit 1 rather than the skill.
const HOW: &str =
    "skills — read one before doing what it covers: `mem skill <name>`, `workflow skill <name>`";

/// `mem skill` with no name lists what this binary owns; with one, prints that
/// SKILL.md whole. Exit 1 is a name mem does not serve.
pub fn cmd_skill(name: Option<&str>) -> i32 {
    let Some(name) = name else {
        print!("{}", own_listing());
        return exit::OK;
    };
    match body(name) {
        Some(text) => {
            print!("{text}");
            exit::OK
        }
        None => {
            eprintln!("mem: skill: no skill '{name}' here — try `workflow skill {name}`");
            exit::NOT_FOUND
        }
    }
}

/// One skill whole, or nothing when mem does not serve that name.
pub fn body(name: &str) -> Option<&'static str> {
    SKILLS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, text)| *text)
}

/// The digest's skills section: the instruction line, mem's own skills, then
/// whatever `workflow skill` printed.
///
/// Empty when there is nothing to name, so a machine without workflow installed
/// and a mem that served nothing add no heading to the digest.
pub fn section() -> String {
    let mut listing = own_listing();
    listing.push_str(&workflow_listing());
    if listing.is_empty() {
        return String::new();
    }
    format!("{HOW}\n{listing}")
}

/// `<name> — <description>` per skill mem owns, name order.
fn own_listing() -> String {
    let mut skills: Vec<&(&str, &str)> = SKILLS.iter().collect();
    skills.sort_by_key(|(name, _)| *name);
    skills
        .iter()
        .map(|(name, text)| format!("{name} — {}\n", description(text)))
        .collect()
}

/// What `workflow skill` printed, verbatim.
///
/// mem does not parse it: the lines are workflow's to shape, and appending them
/// whole means a line that gains a field reaches the session as written. A
/// missing binary, a nonzero exit or empty output is those skills absent —
/// never an error and never a stall, because this runs on the session-start
/// path and nothing there may fail a session.
///
/// `workflow skill` reads embedded text and calls no mem, which is what keeps
/// this from being two binaries shelling out to each other in a circle.
fn workflow_listing() -> String {
    let bin = match std::env::var("WORKFLOW_BIN") {
        Ok(v) if !v.is_empty() => v,
        _ => "workflow".to_string(),
    };
    let Ok(out) = Command::new(bin)
        .arg("skill")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return String::new();
    };
    if !out.status.success() {
        return String::new();
    }
    let mut text = String::from_utf8_lossy(&out.stdout).trim_end().to_string();
    if text.is_empty() {
        return text;
    }
    text.push('\n');
    text
}

/// The `description:` line out of a SKILL.md's YAML frontmatter. A skill
/// without one is still named: the listing is how a session learns it exists.
fn description(text: &str) -> String {
    let mut in_fm = false;
    for (i, line) in text.lines().enumerate() {
        if i == 0 && line == "---" {
            in_fm = true;
            continue;
        }
        if in_fm && line == "---" {
            break;
        }
        if in_fm && let Some(rest) = line.strip_prefix("description:") {
            return rest.trim().to_string();
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mem_skill_is_served_whole_and_named_with_its_description() {
        let listing = own_listing();
        assert_eq!(listing.lines().count(), 1, "{listing}");
        assert!(listing.starts_with("mem — "), "{listing}");
        assert!(!listing.contains("\n\n"));
        assert!(body("mem").is_some_and(|t| t.starts_with("---\nname: mem")));
        assert!(body("route").is_none(), "route is workflow's to serve");
    }

    #[test]
    fn a_workflow_that_is_not_there_costs_only_its_own_skills() {
        // SAFETY: single-threaded assertion on this process's own environment.
        unsafe { std::env::set_var("WORKFLOW_BIN", "/nonexistent/workflow") };
        assert_eq!(workflow_listing(), "");
        let section = section();
        assert!(section.starts_with(HOW), "{section}");
        assert!(section.contains("mem — "), "{section}");
        unsafe { std::env::remove_var("WORKFLOW_BIN") };
    }
}
