//! `workflow worktree`, the Claude Code hook that makes an Agent's or a
//! `claude -w` session's worktree in place of Claude Code's own.
//!
//! The folder goes under the repository's main checkout, at
//! `.claude/worktrees/<name>` on the branch `worktree-<name>`, where the
//! orchestrator finds a subagent's branch. The branch starts at the HEAD of
//! the session's own checkout, the trunk, which is a linked worktree itself
//! when two apps of one monorepo each have one.
//!
//! The worktree then gets the trunk's installs: its root node_modules
//! linked, and every nested node_modules, every vendor and the generated
//! types (`.react-router`, `worker-configuration.d.ts`, `.cloudflare/types`)
//! copied with `cp -a`. A linked nested node_modules resolves a workspace sibling to the
//! trunk's source through its relative links, and a linked vendor broke Pest.
//! A copy that fails is reported and skipped: the worktree still comes up.
//!
//! Removing one keeps any work: git refuses a worktree with uncommitted or
//! untracked files, and `branch -d` a branch that is not merged.
//! Claude Code calls it for a `claude -w` session's worktree, never for a
//! subagent's, which the orchestrator removes once it has landed.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::exit;
use crate::gitcmd::Git;
use crate::warn;

fn hook_input() -> Value {
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    serde_json::from_str(&input).unwrap_or_default()
}

pub fn cmd_worktree_create() -> i32 {
    let hook = hook_input();
    let (Some(name), Some(cwd)) = (hook["name"].as_str(), hook["cwd"].as_str()) else {
        warn("worktree create: the hook input has no name or cwd");
        return exit::FAILED;
    };
    let session = Git::at(cwd);
    let (Some(trunk), Some(main)) = (session.toplevel(), main_checkout(&session)) else {
        warn(format!("worktree create: {cwd} is not in a git checkout"));
        return exit::FAILED;
    };
    let dir = main.join(".claude/worktrees").join(name);
    let branch = format!("worktree-{name}");
    let git = Git::at(&trunk);
    let found = git.quiet(&[
        "rev-parse",
        "--verify",
        "--quiet",
        &format!("refs/heads/{branch}"),
    ]);
    let add = git.capture(&[
        "worktree",
        "add",
        "-b",
        &branch,
        &dir.to_string_lossy(),
        "HEAD",
    ]);
    if !add.ok {
        // git removes a folder it made for an add that failed, but not the
        // branch, which would fail the next add of this name.
        eprint!("{}", String::from_utf8_lossy(&add.stderr));
        if !found {
            git.quiet(&["branch", "-D", &branch]);
        }
        return exit::FAILED;
    }
    bring_installs(&trunk, &dir);
    println!("{}", dir.display());
    exit::OK
}

pub fn cmd_worktree_remove() -> i32 {
    let hook = hook_input();
    let Some(path) = hook["worktree_path"].as_str() else {
        warn("worktree remove: the hook input has no worktree_path");
        return exit::FAILED;
    };
    let worktree = Git::at(path);
    let (Some(branch), Some(main)) = (
        worktree.out(&["branch", "--show-current"]),
        main_checkout(&worktree),
    ) else {
        warn(format!(
            "worktree remove: {path} is not a worktree on a branch"
        ));
        return exit::FAILED;
    };
    let main = Git::at(main);
    let removed = main.capture(&["worktree", "remove", path]);
    if !removed.ok {
        eprint!("{}", String::from_utf8_lossy(&removed.stderr));
        warn(format!(
            "worktree remove: kept {path}, which holds uncommitted work"
        ));
        return exit::FAILED;
    }
    if !main.quiet(&["branch", "-d", &branch]) {
        warn(format!(
            "worktree remove: kept the branch {branch}, which is not merged"
        ));
    }
    exit::OK
}

/// Link the trunk's root node_modules into `dir` and copy the rest: each
/// folder with a tracked package.json or composer.json may hold its own.
fn bring_installs(trunk: &Path, dir: &Path) {
    if trunk.join("node_modules").exists() {
        let link = dir.join("node_modules");
        if let Err(e) = std::os::unix::fs::symlink(trunk.join("node_modules"), &link) {
            warn(format!("worktree create: {}: {e}", link.display()));
        }
    }
    for package in package_dirs(dir) {
        // The root node_modules is the link above.
        let root = package.as_os_str().is_empty();
        let nested = ["node_modules"].iter().filter(|_| !root);
        for name in nested.chain(COPIED) {
            copy(
                &trunk.join(&package).join(name),
                &dir.join(&package).join(name),
            );
        }
    }
}

/// What each package folder's copy gets from the trunk's besides its
/// node_modules: vendor, and the generated types a typecheck reads before
/// anything regenerates them.
const COPIED: &[&str] = &[
    "vendor",
    ".react-router",
    "worker-configuration.d.ts",
    ".cloudflare/types",
];

/// The folders, relative to the worktree, of its tracked package.json and
/// composer.json files; the root is the empty path.
fn package_dirs(dir: &Path) -> std::collections::BTreeSet<PathBuf> {
    let listed = Git::at(dir).bytes(&["ls-files", "-z"]);
    listed
        .split(|&b| b == 0)
        .map(|path| Path::new(std::str::from_utf8(path).unwrap_or_default()))
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name == "package.json" || name == "composer.json")
        })
        .filter_map(|path| path.parent().map(Path::to_path_buf))
        .collect()
}

/// `cp -a from to`, when the trunk has `from` and the worktree has no `to`,
/// so a tracked file of the same name is never overwritten.
fn copy(from: &Path, to: &Path) {
    if !from.exists() || std::fs::symlink_metadata(to).is_ok() {
        return;
    }
    if let Some(parent) = to.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let copied = std::process::Command::new("cp")
        .arg("-a")
        .arg(from)
        .arg(to)
        .status()
        .is_ok_and(|status| status.success());
    if !copied {
        warn(format!(
            "worktree create: could not copy {}",
            from.display()
        ));
    }
}

/// The main checkout, the folder that holds the common git dir, with no
/// symlink in its path: Claude Code refuses a worktree path through one.
fn main_checkout(git: &Git) -> Option<PathBuf> {
    git.common_dir()?.parent()?.canonicalize().ok()
}
