//! `workflow skill` -- the skills this binary carries, served rather than
//! installed.
//!
//! A skill used to be a file on disk. `doctor --fix` wrote eight of them into
//! every directory a harness auto-discovers, which meant a settings file per
//! harness to gate them (Claude's `skillOverrides`, pi's `skills` array, two
//! grammars and a trust rule) and a doctor check to catch a copy that had
//! drifted from the binary. Text read out of `include_str!` cannot drift from
//! the binary reading it, and there is nothing on disk to gate: mem's digest
//! names these where mem knows the project, and that is the whole switch.
//!
//! Nothing here reads a store, runs git or calls mem. It must not: `mem
//! context` shells out to `workflow skill` to name the seven it does not own,
//! so a call back the other way is how two binaries find a way to recurse.

use crate::exit;

/// The skills this binary owns, embedded so a copied binary carries them
/// wherever it runs.
///
/// mem is not here. It ships `skills/mem/SKILL.md` inside its own binary and
/// serves it as `mem skill mem`: a skill belongs to the binary it is about, so
/// a machine with mem and no workflow still has the skill that explains mem.
pub const SKILLS: [(&str, &str); 7] = [
    ("implement", include_str!("../../skills/implement/SKILL.md")),
    (
        "orchestrate",
        include_str!("../../skills/orchestrate/SKILL.md"),
    ),
    ("plan", include_str!("../../skills/plan/SKILL.md")),
    ("review", include_str!("../../skills/review/SKILL.md")),
    ("roadmap", include_str!("../../skills/roadmap/SKILL.md")),
    ("route", include_str!("../../skills/route/SKILL.md")),
    ("unslop", include_str!("../../skills/unslop/SKILL.md")),
];

/// The name mem serves, so an ask for it can say where to go instead of
/// reporting a skill that does not exist.
const MEM_SKILL: &str = "mem";

/// `workflow skill` with no name lists what this binary owns; with one, prints
/// that SKILL.md whole. Exit 1 is a name nobody here serves -- the same code a
/// filtered read that matched nothing uses.
pub fn cmd_skill(name: Option<&str>) -> i32 {
    let Some(name) = name else {
        print!("{}", listing());
        return exit::OK;
    };
    if let Some((_, text)) = SKILLS.iter().find(|(n, _)| *n == name) {
        print!("{text}");
        return exit::OK;
    }
    if name == MEM_SKILL {
        eprintln!("workflow: skill: mem serves its own skill -- `mem skill mem`");
    } else {
        eprintln!("workflow: skill: no skill '{name}' here -- `workflow skill` lists them");
    }
    exit::FAILED
}

/// One `<name> — <description>` line per skill, name order. This is what mem
/// appends to its digest verbatim, so the shape is the contract: mem does not
/// parse it, and a line that gained a field would be a line mem shows as is.
pub fn listing() -> String {
    let mut names: Vec<&(&str, &str)> = SKILLS.iter().collect();
    names.sort_by_key(|(name, _)| *name);
    names
        .iter()
        .map(|(name, text)| format!("{name} — {}\n", description(text)))
        .collect()
}

/// The `description:` line out of a SKILL.md's YAML frontmatter, as one line.
/// A skill without one is named with an empty description rather than dropped:
/// the listing is how a model learns the skill exists at all.
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
    fn every_skill_carries_a_description_on_one_line() {
        for (name, text) in SKILLS {
            let d = description(text);
            assert!(!d.is_empty(), "{name} has no description");
            assert!(!d.contains('\n'), "{name}'s description wraps");
        }
    }

    #[test]
    fn the_listing_is_one_line_per_skill_in_name_order() {
        let listing = listing();
        let lines: Vec<&str> = listing.lines().collect();
        assert_eq!(lines.len(), SKILLS.len());
        let mut sorted = lines.clone();
        sorted.sort();
        assert_eq!(lines, sorted);
        assert!(lines.iter().all(|l| l.contains(" — ")), "{lines:?}");
    }
}
