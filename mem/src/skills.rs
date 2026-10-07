//! The skill mem owns, served by `mem skill`.
//!
//! The skills a harness opens are ordinary files `workflow install` writes into
//! its skills directory, and the harness lists them itself. This is the one
//! skill that also lives in mem's binary, so a machine with mem and no
//! workflow can still read how mem is used.

use crate::exit;

/// The skill mem owns, embedded so the binary carries it wherever it runs.
pub const SKILLS: [(&str, &str); 1] = [("mem", include_str!("../../skills/mem/SKILL.md"))];

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
            eprintln!("mem: skill: no skill '{name}' here — mem serves only `mem`");
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

/// `<name> — <description>` per skill mem owns, name order.
fn own_listing() -> String {
    let mut skills: Vec<&(&str, &str)> = SKILLS.iter().collect();
    skills.sort_by_key(|(name, _)| *name);
    skills
        .iter()
        .map(|(name, text)| format!("{name} — {}\n", description(text)))
        .collect()
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
        assert!(body("route").is_none());
    }
}
