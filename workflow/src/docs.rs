//! `workflow docs <library> "<query>"`: a library's current documentation,
//! through the Context7 CLI, for a worker that is unsure of an API.
//!
//! A worker with no docs channel reads a package's built `dist` to learn
//! its shape: ebdify m1's gql worker spent 84 of 107 calls in
//! `node_modules/.pnpm` on Pothos, Yoga and gql.tada internals, and
//! narrated a docs tool pi never exposed (m1-lessons ruling 8). This is
//! the channel: `npx ctx7@latest library <name> "<query>"` for the first
//! Context7 id, then `npx ctx7@latest docs <id> "<query>"` for the text,
//! printed as it came. Exit 1 with the CLI's own stderr when nothing came.

use std::process::Command;

use crate::{exit, warn};

/// The first `Context7-compatible library ID:` in a `library` listing.
pub fn first_id(listing: &str) -> Option<String> {
    listing
        .lines()
        .find_map(|l| l.trim().strip_prefix("Context7-compatible library ID:"))
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
}

fn ctx7(args: &[&str]) -> Result<String, String> {
    let out = Command::new("npx")
        .arg("-y")
        .arg("ctx7@latest")
        .args(args)
        .output()
        .map_err(|e| format!("cannot run npx: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if !out.status.success() {
        return Err(match stderr.is_empty() {
            true => format!("ctx7 {} exited {}", args[0], out.status),
            false => stderr,
        });
    }
    Ok(stdout)
}

pub fn cmd_docs(library: &str, query: &str) -> i32 {
    let library = library.trim();
    let query = query.trim();
    if library.is_empty() || query.is_empty() {
        warn("docs: a library and a query: workflow docs <library> \"<query>\"");
        return exit::USAGE;
    }
    let listing = match ctx7(&["library", library, query]) {
        Ok(text) => text,
        Err(why) => {
            warn(format!("docs: {why}"));
            return exit::FAILED;
        }
    };
    let Some(id) = first_id(&listing) else {
        warn(format!(
            "docs: no library matched '{library}' -- try another name"
        ));
        return exit::FAILED;
    };
    match ctx7(&["docs", &id, query]) {
        Ok(text) if !text.trim().is_empty() => {
            warn(format!("docs: {id}"));
            print!("{text}");
            exit::OK
        }
        Ok(_) => {
            warn(format!("docs: {id} answered nothing for '{query}'"));
            exit::FAILED
        }
        Err(why) => {
            warn(format!("docs: {why}"));
            exit::FAILED
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_id_is_read_off_the_listing() {
        let listing = "1. Title: Pothos GraphQL\n   Context7-compatible library ID: /hayes/pothos\n   Description: x\n\n2. Title: Pothos\n   Context7-compatible library ID: /websites/pothos-graphql_dev\n";
        assert_eq!(first_id(listing).as_deref(), Some("/hayes/pothos"));
        assert_eq!(first_id("No libraries found.\n"), None);
    }
}
