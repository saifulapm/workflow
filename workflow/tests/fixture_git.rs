//! Every Rust test's own git runs without the global hooks. From inside an
//! agent session those hooks see the agent's variables and refuse a
//! fixture's push or its task-shaped commit messages, which turned ten
//! go_cloud tests red on a green trunk (0c01e82).

use std::fs;
use std::path::{Path, PathBuf};

const CRATES: [&str; 5] = ["mem", "hub", "hubpages", "hubweb", "workflow"];

/// Either form keeps the global hooks out: no hooks path at all, or no
/// global config to name one.
const NO_HOOKS: [&str; 2] = ["core.hooksPath=/dev/null", "GIT_CONFIG_GLOBAL"];

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn every_test_git_runs_without_the_global_hooks() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut files = Vec::new();
    for krate in CRATES {
        rust_files(&repo.join(krate).join("tests"), &mut files);
    }
    let mut bare = Vec::new();
    for file in &files {
        let text = fs::read_to_string(file).unwrap();
        for (at, _) in text.match_indices("Command::new(\"git\")") {
            // The builder chain, up to the call that runs it.
            let rest = &text[at..];
            let end = [".output()", ".status()", ".spawn()"]
                .iter()
                .filter_map(|run| rest.find(run))
                .min()
                .unwrap_or(rest.len());
            if !NO_HOOKS.iter().any(|form| rest[..end].contains(form)) {
                let line = text[..at].matches('\n').count() + 1;
                bare.push(format!(
                    "{}:{line}",
                    file.strip_prefix(repo).unwrap().display()
                ));
            }
        }
    }
    assert!(
        bare.is_empty(),
        "a test's git runs the global hooks; pass `-c core.hooksPath=/dev/null`:\n{}",
        bare.join("\n")
    );
}
