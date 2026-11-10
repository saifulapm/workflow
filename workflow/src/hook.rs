//! `workflow hook <name>` -- the body of a git hook stub.
//!
//! Four steps in this order. Two properties are load-bearing and were found the
//! hard way:
//!
//!   * the non-firing path never exits early. Every branch reaches step 4,
//!     because a global `core.hooksPath` puts this between every human commit on
//!     the machine and the repo's own hooks. Exiting early would silently
//!     disable husky. The one exit is a refusal: a human commit in a checkout
//!     mem knows still gets the hygiene check.
//!   * the hook never touches its own environment. git sets `GIT_DIR` and
//!     `GIT_INDEX_FILE` for hooks, and a partial commit's staged view lives in
//!     that temporary index; unsetting them blinds the staged-diff checks and
//!     breaks whatever we chain into.
//!
//! Nothing here runs a project's tests: pre-commit reads the staged diff for
//! hygiene and commit-msg reads the message, for humans and agents alike.

use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::gitcmd::{self, Git};
use crate::{exit, hygiene, memcli, paths, warn};

/// Step 2, the fire condition: an agent standing in a checkout mem knows. The
/// env markers are inherited by everything an agent starts, so the env branch
/// alone would gate every scratch repo a test suite creates.
fn fires() -> bool {
    agent_marked() && memcli::knows_this_checkout()
}

/// Is an agent driving this commit? `WORKFLOW_AGENT` is ours, set by the Claude
/// Code settings file. `PI_CODING_AGENT` is pi's own, which pi exports to every
/// process it starts: pi has no `env` key in its settings, so it is the only
/// marker a hand-started pi session carries.
pub fn agent_marked() -> bool {
    ["WORKFLOW_AGENT", "PI_CODING_AGENT"]
        .iter()
        .any(|key| std::env::var(key).is_ok_and(|v| !v.is_empty()))
}

/// Step 3, the check itself. A pre-push check in a cloud thread reads the ref
/// lines git feeds on stdin and leaves them in `fed`, for step 4 to hand on.
fn check(name: &str, args: &[String], fed: &mut Option<Vec<u8>>) -> i32 {
    match name {
        "pre-commit" | "commit-msg" => words(name, args),
        "pre-push" => {
            if std::env::var("WORKFLOW_ALLOW_PUSH").unwrap_or_default() == "1" {
                return exit::OK;
            }
            if crate::cloud::this_session().is_some() {
                let mut input = Vec::new();
                let _ = std::io::stdin().read_to_end(&mut input);
                let cloud_only = only_cloud_refs(&String::from_utf8_lossy(&input));
                *fed = Some(input);
                if cloud_only {
                    return exit::OK;
                }
            }
            warn("push requires explicit approval.");
            warn("with a human present and their yes: WORKFLOW_ALLOW_PUSH=1 git push");
            exit::FAILED
        }
        other => {
            warn(format!("hook: nothing to check for '{other}'"));
            exit::OK
        }
    }
}

/// Does every line git fed pre-push send to a `cloud/` branch? A thread's
/// agent owns those and nothing else. The destination is the third field, so
/// `git push origin HEAD:main` from a cloud/ branch still counts as main. No
/// line at all is a push with nothing to send, which moves no branch.
fn only_cloud_refs(input: &str) -> bool {
    input.lines().filter(|l| !l.trim().is_empty()).all(|l| {
        l.split_whitespace()
            .nth(2)
            .is_some_and(|r| r.starts_with("refs/heads/cloud/"))
    })
}

/// The hygiene check: the staged diff before the commit, the message after it
/// is written.
fn words(name: &str, args: &[String]) -> i32 {
    let message = match (name, args.first()) {
        ("pre-commit", _) => None,
        ("commit-msg", Some(f)) => Some(Path::new(f)),
        _ => return exit::OK,
    };
    let scope = hygiene::Scope {
        staged: message.is_none(),
        tree: false,
        history: None,
        message,
        string: None,
    };
    hygiene::cmd_hygiene(scope, None, false, false)
}

/// Step 4: chain to the repo's own hook. The stub's path decides whether that
/// hook *is* this stub, which would exec in a circle.
/// When step 3 read stdin, `fed` holds those bytes and the repo's hook gets
/// them on its own stdin, so a husky pre-push still sees the refs.
fn chain(name: &str, hd: &Path, stub: Option<&Path>, args: &[String], fed: Option<Vec<u8>>) -> i32 {
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

    if let Some(input) = fed {
        return feed(&own, args, &input);
    }
    let err = Command::new(&own).args(args).exec();
    warn(format!("cannot run {}: {err}", own.display()));
    exit::FAILED
}

/// Run the repo's hook with `input` on its stdin and return its exit code. A
/// hook that never reads stdin closes the pipe; that write error is not ours.
fn feed(own: &Path, args: &[String], input: &[u8]) -> i32 {
    let mut child = match Command::new(own).args(args).stdin(Stdio::piped()).spawn() {
        Ok(c) => c,
        Err(err) => {
            warn(format!("cannot run {}: {err}", own.display()));
            return exit::FAILED;
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input);
    }
    match child.wait() {
        Ok(status) => status.code().unwrap_or(exit::FAILED),
        Err(err) => {
            warn(format!("cannot run {}: {err}", own.display()));
            exit::FAILED
        }
    }
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

    // 2 and 3.
    let mut fed = None;
    if fires() {
        let rc = check(name, args, &mut fed);
        if rc != exit::OK {
            return rc;
        }
    } else if !agent_marked() && memcli::knows_this_checkout() {
        // A human commit in a checkout mem knows: hygiene. This is the only
        // path WORKFLOW_HYGIENE=skip reaches; the firing path is an agent's,
        // and nothing clears it there.
        if std::env::var("WORKFLOW_HYGIENE").unwrap_or_default() == "skip" {
            warn(format!(
                "hook: {name}: hygiene skipped by WORKFLOW_HYGIENE=skip"
            ));
        } else {
            let rc = words(name, args);
            if rc != exit::OK {
                return rc;
            }
        }
    }

    // 4.
    chain(name, &hd, stub, args, fed)
}
