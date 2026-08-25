//! The checkout half of `workflow plan-check` (frictions #DBHZBFY1 and
//! #6485CNC0): a plan is judged against the tree it will run in, not only
//! against its own grammar. Everything here is knowable before dispatch, and
//! each finding used to cost a worker a whole attempt to rediscover.
//!
//! Refusals are Verify lines that cannot pass in this checkout. Warnings are
//! Files lines that do not look like they can hold their task -- warnings,
//! because a plan can mean to create what is not there yet.

use std::path::Path;

use crate::gitcmd::{self, Git};
use crate::ownership;
use crate::plan::Plan;

pub struct Findings {
    pub refusals: Vec<String>,
    pub warnings: Vec<String>,
}

pub fn findings(plan: &Plan, root: &Path) -> Findings {
    let git = Git::at(root);
    let mut f = Findings {
        refusals: Vec::new(),
        warnings: Vec::new(),
    };
    for t in &plan.tasks {
        if t.checked {
            continue; // never dispatched, so its lines are history, not risk
        }
        let verify = t.verify.as_deref().unwrap_or("");
        if let Some(msg) = lib_test_without_lib(&t.id, verify, root) {
            f.refusals.push(msg);
        }
        let patterns = ownership::split_patterns(t.files.as_deref().unwrap_or(""));
        for p in &patterns {
            if matches_nothing(&git, root, p) {
                f.warnings.push(format!(
                    "plan: task {}: '{p}' matches nothing here and its directory does not exist -- a task creating it, or a typo",
                    t.id
                ));
            }
        }
        if runs_tests(verify) && !patterns.iter().any(|p| p.to_lowercase().contains("test")) {
            f.warnings.push(format!(
                "plan: task {}: its Verify runs tests and its Files list no test file -- the worker cannot add the test that proves it",
                t.id
            ));
        }
    }
    f
}

/// `cargo test --lib` in a crate with no library target runs nothing and can
/// never go green -- the trap four tasks each paid an attempt to find
/// (friction #DBHZBFY1). A workspace manifest is left alone: the member that
/// Verify would run in is not knowable from here.
fn lib_test_without_lib(task: &str, verify: &str, root: &Path) -> Option<String> {
    if !verify.contains("cargo test") || !verify.split_whitespace().any(|w| w == "--lib") {
        return None;
    }
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).ok()?;
    if manifest.contains("[workspace]") || manifest.contains("[lib]") {
        return None;
    }
    if root.join("src/lib.rs").is_file() {
        return None;
    }
    Some(format!(
        "plan: task {task}: Verify runs 'cargo test --lib' and this crate has no library target -- it can never pass here"
    ))
}

/// Nothing tracked matches the pattern, the literal path is not there, and
/// neither is the directory it points into. One of those three usually holds
/// even for a file the task will create; when none does, the pattern is
/// probably not naming what the author thought.
fn matches_nothing(git: &Git, root: &Path, pattern: &str) -> bool {
    if root.join(pattern).exists() {
        return false;
    }
    let spec = gitcmd::glob_top(pattern);
    if !git.bytes(&["ls-files", "-z", "--", &spec]).is_empty() {
        return false;
    }
    let fixed: String = pattern
        .chars()
        .take_while(|c| !matches!(c, '*' | '?' | '['))
        .collect();
    let dir = match fixed.rsplit_once('/') {
        Some((d, _)) => root.join(d),
        None => root.to_path_buf(),
    };
    !dir.is_dir()
}

fn runs_tests(verify: &str) -> bool {
    verify.contains("test")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_test_run_is_recognised_by_the_word_and_true_is_not() {
        assert!(runs_tests("cargo test --test e2e"));
        assert!(runs_tests("pnpm run test:unit"));
        assert!(runs_tests("bin/php artisan test --filter=Cart"));
        assert!(!runs_tests("true"));
        assert!(!runs_tests("cargo build"));
    }
}
