//! `workflow guard agent`: an implementer's brief whose VERIFY names no lint
//! lands code the trunk then has to clean (palmshop m19 t3 brought four
//! lint errors, as a brief had on 2026-11-02).

use std::path::Path;

use super::by_path;
use super::pnpm::runs_pnpm;
use crate::shell;

pub const NO_LINT: &str = "workflow guard refused this on purpose: this implementer brief's VERIFY names no lint, so its commits can land lint errors on the trunk.\n\
End VERIFY with the app's lint and format, as in `node_modules/.bin/oxlint <dir>` or `cargo clippy -q --all-targets -- -D warnings`.";

/// The headings an orchestrator's brief is written under.
const HEADINGS: &[&str] = &[
    "GOAL",
    "SCOPE",
    "SETUP",
    "CONTEXT",
    "ACCEPTANCE",
    "VERIFY",
    "STOP",
];

/// The words a lint command holds: oxlint, eslint and `pnpm lint` all say
/// lint.
const LINTS: &[&str] = &["lint", "clippy", "biome", "ruff", "shellcheck"];

/// Whether `prompt`, an implementer's brief, has a VERIFY section that
/// names a lint.
pub fn verify_names_a_lint(prompt: &str) -> bool {
    verify_lines(prompt).iter().any(|line| {
        let line = line.to_lowercase();
        LINTS.iter().any(|lint| line.contains(lint))
    })
}

pub const RUNS_PNPM: &str = "workflow guard refused this on purpose: this implementer brief's VERIFY runs pnpm, which its worktree's linked node_modules refuses.";

/// The refusal of `prompt`, an implementer's brief written in `cwd`, when
/// its VERIFY runs pnpm: each such line's by-path form, or how to write it
/// when one has none.
pub fn pnpm_refusal(prompt: &str, cwd: &Path) -> Option<String> {
    let mut lines = Vec::new();
    let mut named = true;
    for line in verify_lines(prompt) {
        let Some(commands) = shell::commands(line) else {
            continue;
        };
        if !commands.iter().any(|c| runs_pnpm(&c.words)) {
            continue;
        }
        match by_path::lines(&commands, cwd) {
            Some(by_path) => lines.extend(by_path),
            None => named = false,
        }
    }
    if named && lines.is_empty() {
        None
    } else if named {
        Some(format!(
            "{RUNS_PNPM} Write these lines in its place, each a call of its own:\n{}",
            lines.join("\n")
        ))
    } else {
        Some(format!(
            "{RUNS_PNPM}\nCall each tool by path instead, as in node_modules/.bin/vitest run <file>, one line each."
        ))
    }
}

/// The lines of `prompt`'s VERIFY section, the heading's own line after
/// the heading, each without its list mark and backticks.
fn verify_lines(prompt: &str) -> Vec<&str> {
    let mut in_verify = false;
    let mut out = Vec::new();
    for line in prompt.lines() {
        let mut line = line.trim();
        let first = line.split_whitespace().next().unwrap_or_default();
        if HEADINGS.contains(&first) {
            in_verify = first == "VERIFY";
            line = line[first.len()..].trim_start();
        }
        if in_verify {
            let line = line
                .strip_prefix("- ")
                .or_else(|| line.strip_prefix("* "))
                .unwrap_or(line);
            out.push(line.trim_matches('`'));
        }
    }
    out
}
