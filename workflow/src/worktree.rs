//! `workflow worktree`, the Claude Code hook that makes an Agent's or a
//! `claude -w` session's worktree in place of Claude Code's own.
//!
//! The folder goes under the repository's main checkout, at
//! `.claude/worktrees/<name>` on the branch `worktree-<name>`, where the
//! orchestrator finds a subagent's branch. The branch starts at the HEAD of
//! the session's own checkout, the trunk, which is a linked worktree itself
//! when two apps of one monorepo each have one.

use std::io::Read;
use std::path::PathBuf;

use serde_json::Value;

use crate::exit;
use crate::gitcmd::Git;
use crate::warn;

pub fn cmd_worktree_create() -> i32 {
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    let hook: Value = serde_json::from_str(&input).unwrap_or_default();
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
    let add = Git::at(&trunk).capture(&[
        "worktree",
        "add",
        "-b",
        &branch,
        &dir.to_string_lossy(),
        "HEAD",
    ]);
    if !add.ok {
        // git removes a folder it made for an add that failed.
        eprint!("{}", String::from_utf8_lossy(&add.stderr));
        return exit::FAILED;
    }
    println!("{}", dir.display());
    exit::OK
}

/// The main checkout, the folder that holds the common git dir, with no
/// symlink in its path: Claude Code refuses a worktree path through one.
fn main_checkout(git: &Git) -> Option<PathBuf> {
    git.common_dir()?.parent()?.canonicalize().ok()
}
