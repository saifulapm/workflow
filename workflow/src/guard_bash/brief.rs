//! `workflow guard agent`: an implementer's brief whose VERIFY names no lint
//! lands code the trunk then has to clean (palmshop m19 t3 brought four
//! lint errors, as a brief had on 2026-11-02).

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
    let mut in_verify = false;
    let mut found = false;
    for line in prompt.lines() {
        let first = line.split_whitespace().next().unwrap_or_default();
        if HEADINGS.contains(&first) {
            in_verify = first == "VERIFY";
        }
        if in_verify {
            let line = line.to_lowercase();
            found |= LINTS.iter().any(|lint| line.contains(lint));
        }
    }
    found
}
