//! `workflow hook <name>` -- the body of a git hook stub (spec §7, §12b).
//!
//! Five steps in this order. Two properties are load-bearing and were found the
//! hard way:
//!
//!   * the non-firing path never exits early. Every branch reaches step 5,
//!     because a global `core.hooksPath` puts this between every human commit on
//!     the machine and the repo's own hooks. Exiting early would silently
//!     disable husky.
//!   * the hook never touches its own environment. git sets `GIT_DIR` and
//!     `GIT_INDEX_FILE` for hooks, and a partial commit's staged view lives in
//!     that temporary index; unsetting them blinds the staged-diff checks and
//!     breaks whatever we chain into. Only the child test suite is scrubbed,
//!     inside verify.

use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;

use crate::gitcmd::{self, Git};
use crate::{exit, lint, memcli, paths, verify, warn};

/// Step 2, the fire condition: a location-sane union (review-4 B-2). An
/// orchestrator worktree, or an agent standing in a checkout mem knows.
/// `WORKFLOW_AGENT` is inherited by everything an agent starts, so the env
/// branch alone would gate every scratch repo a test suite creates.
fn fires(git: &Git) -> bool {
    if let Some(top) = git.toplevel()
        && paths::under_worktrees_root(&top)
    {
        return true;
    }
    match std::env::var("WORKFLOW_AGENT") {
        Ok(v) if !v.is_empty() => memcli::knows_this_checkout(),
        _ => false,
    }
}

/// Step 4, the check itself.
fn check(name: &str, args: &[String]) -> i32 {
    match name {
        "pre-commit" => verify::cmd_verify(verify::Mode::Hook),
        "commit-msg" => match args.first() {
            Some(f) => lint::cmd_lint_msg(Some(Path::new(f)), None),
            None => exit::OK,
        },
        "pre-push" => {
            if std::env::var("WORKFLOW_ALLOW_PUSH").unwrap_or_default() == "1" {
                exit::OK
            } else {
                warn("push requires explicit approval.");
                warn("with a human present and their yes: WORKFLOW_ALLOW_PUSH=1 git push");
                exit::FAILED
            }
        }
        other => {
            warn(format!("hook: nothing to check for '{other}'"));
            exit::OK
        }
    }
}

/// Step 5: chain to the repo's own hook. The stub's path decides whether that
/// hook *is* this stub, which would exec in a circle.
fn chain(name: &str, hd: &Path, stub: Option<&Path>, args: &[String], seen: Option<&str>) -> i32 {
    let own = hd.join("hooks").join(name);
    if !gitcmd::exists_x(&own) {
        return exit::OK;
    }
    let me = match stub {
        Some(p) => paths::realpath(p),
        None => std::env::current_exe().ok().and_then(paths::realpath),
    };
    if paths::realpath(&own) == me && me.is_some() {
        return exit::OK;
    }

    let mut c = Command::new(&own);
    c.args(args);
    if let Some(hd) = seen {
        c.env("WORKFLOW_HOOK_SEEN", hd);
    }
    let err = c.exec();
    warn(format!("cannot run {}: {err}", own.display()));
    exit::FAILED
}

pub fn cmd_hook(name: &str, stub: Option<&Path>, args: &[String]) -> i32 {
    let git = Git::here();

    // 1. A work tree, or nothing to do. The stub has already asked, but this
    //    command is also reachable by hand.
    if !git.inside_worktree() {
        return exit::OK;
    }
    let Some(hd) = git.common_dir() else {
        return exit::OK;
    };
    let hd_key = hd.to_string_lossy().to_string();

    // 2 and 3. The depth guard is per repo and only on the firing path: it
    //    stops recursion; it does not make a nested same-repo commit safe -- a
    //    suite that commits in the repo being committed still races the outer
    //    index.
    let mut seen: Option<String> = None;
    if fires(&git) {
        seen = Some(hd_key.clone());
        if std::env::var("WORKFLOW_HOOK_SEEN").unwrap_or_default() != hd_key {
            // 4.
            let rc = check(name, args);
            if rc != exit::OK {
                return rc;
            }
        }
    }

    // 5.
    chain(name, &hd, stub, args, seen.as_deref())
}
