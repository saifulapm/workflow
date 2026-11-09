//! `workflow land`: a thread of a Claude Project lands its milestone on the
//! trunk itself. It rebases the milestone branch on origin's main, runs the
//! project's test command, pushes the branch to main as a fast-forward,
//! never forced, ticks the milestone, and prints the line the thread ends
//! its report on, which its coordinator reads to start the next thread.

use std::process::{Command, Stdio};

use crate::cloud;
use crate::gitcmd::Git;
use crate::memcli;
use crate::{exit, go, warn};

/// The trunk a cloud run lands on.
const TRUNK: &str = "main";

/// How many times a push the trunk refused, because it moved while the
/// tests ran, starts the landing over before the run parks.
const TRIES: usize = 3;

pub fn cmd_land() -> i32 {
    let Some(current) = memcli::project_current() else {
        warn("land: this directory is in no project mem knows");
        return exit::USAGE;
    };
    let project = current.name.clone();
    let Some(projects) = memcli::projects() else {
        warn("land: mem cannot list its projects");
        return exit::FAILED;
    };
    let Some(row) = projects.iter().find(|row| row.name == project) else {
        warn(format!("land: mem lists no {project}"));
        return exit::FAILED;
    };
    let Some(slug) = row.milestone.clone() else {
        println!("done: {project}'s roadmap is done");
        return exit::OK;
    };
    let git = Git::here();
    let Some(dir) = git.toplevel() else {
        warn("land: this directory is no git checkout");
        return exit::USAGE;
    };
    let git = Git::at(&dir);
    let branch = cloud::branch(&project, &slug);
    if git.out(&["branch", "--show-current"]).as_deref() != Some(branch.as_str()) {
        warn(format!(
            "land: a milestone lands from its branch {branch}; check it out first"
        ));
        return exit::USAGE;
    }
    if git.out(&["status", "--porcelain"]).is_some() {
        warn("land: commit or drop what is not committed first; the trunk takes commits only");
        return exit::USAGE;
    }
    let Some(verify) = memcli::project_choice("verify") else {
        warn(format!(
            "land: {project} has no test command; `mem project set verify \"<command>\"`"
        ));
        return exit::USAGE;
    };

    let mut tries = 0;
    loop {
        tries += 1;
        let fetched = git.capture(&["fetch", "-q", "origin", TRUNK]);
        if !fetched.ok {
            warn(format!(
                "land: cannot fetch origin's {TRUNK}: {}",
                String::from_utf8_lossy(&fetched.stderr).trim()
            ));
            return exit::FAILED;
        }
        let trunk = format!("origin/{TRUNK}");
        let base = git.out(&["rev-parse", &trunk]).unwrap_or_default();
        let before = git.out(&["rev-parse", "HEAD"]).unwrap_or_default();
        if !git.capture(&["rebase", "-q", &trunk]).ok {
            let files = git
                .out(&["diff", "--name-only", "--diff-filter=U"])
                .unwrap_or_default()
                .lines()
                .collect::<Vec<_>>()
                .join(", ");
            git.quiet(&["rebase", "--abort"]);
            println!(
                "parked: trunk moved: {} conflict with {slug}'s commits on {branch}; nothing was pushed",
                if files.is_empty() {
                    "its commits"
                } else {
                    &files
                }
            );
            return exit::USAGE;
        }
        // Each task pushed the branch before the rebase rewrote it; origin's
        // copy follows, with a lease on the tip it last had, so the thread's
        // next push of the branch is a fast-forward whatever comes next.
        if git.out(&["rev-parse", "HEAD"]).unwrap_or_default() != before
            && let Err(why) = push_branch(&git, &dir, &branch)
        {
            warn(format!(
                "land: cannot push {branch} rebased onto {trunk}: {why}; nothing was pushed to {TRUNK}"
            ));
            return exit::FAILED;
        }
        let ran = Command::new("bash")
            .args(["-c", &verify])
            .current_dir(&dir)
            .stdin(Stdio::null())
            .stdout(std::io::stderr())
            .status();
        if !ran.is_ok_and(|status| status.success()) {
            warn(format!(
                "land: the tests failed on {branch} rebased onto {trunk}; nothing was pushed. \
Fix it on {branch} and land again"
            ));
            return exit::FAILED;
        }
        let why = match push(&dir, &format!("HEAD:{TRUNK}"), None) {
            Ok(()) => break,
            Err(why) => why,
        };
        // Refused because the trunk moved while the tests ran, the landing
        // starts over; for any other reason it stops here, saying why.
        let moved = git.quiet(&["fetch", "-q", "origin", TRUNK])
            && git.out(&["rev-parse", &trunk]).unwrap_or_default() != base;
        if !moved {
            warn(format!(
                "land: cannot push {branch} to origin's {TRUNK}: {why}; nothing was pushed"
            ));
            return exit::FAILED;
        }
        if tries == TRIES {
            println!(
                "parked: trunk moved: origin's {TRUNK} kept moving while {slug} was tested; \
nothing was pushed"
            );
            return exit::USAGE;
        }
    }

    if !memcli::tick(&project, &slug) {
        warn(format!(
            "land: {slug} is on origin's {TRUNK}, but mem would not tick it; run `mem roadmap --tick {slug}`"
        ));
        return exit::FAILED;
    }
    carry_outbox(&git, &dir);
    println!("{}", next_line(&project));
    exit::OK
}

/// `git push origin <refspec>`, never forced but by `lease`, a
/// `--force-with-lease` that holds only while origin's ref is what it last
/// was. The push hook lets an agent's push through only with this variable:
/// a green test run, or a branch of the thread's own, is the yes. On a
/// refusal it says git's reason.
fn push(dir: &std::path::Path, refspec: &str, lease: Option<&str>) -> Result<(), String> {
    let mut args = vec!["push".to_string(), "-q".to_string()];
    args.extend(lease.map(str::to_string));
    args.extend(["origin".to_string(), refspec.to_string()]);
    let out = Command::new("git")
        .args(&args)
        .env("WORKFLOW_ALLOW_PUSH", "1")
        .current_dir(dir)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| format!("cannot run git: {err}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Push the rebased milestone `branch` over origin's copy, leased on the
/// tip origin holds now, or on its having none.
fn push_branch(git: &Git, dir: &std::path::Path, branch: &str) -> Result<(), String> {
    let refname = format!("refs/heads/{branch}");
    let listed = git.capture(&["ls-remote", "origin", &refname]);
    if !listed.ok {
        return Err(String::from_utf8_lossy(&listed.stderr).trim().to_string());
    }
    let tip = listed.text();
    let tip = tip.split_whitespace().next().unwrap_or("");
    let lease = format!("--force-with-lease={refname}:{tip}");
    push(dir, &format!("HEAD:{refname}"), Some(&lease))
}

/// The tick and the writes before it may not have reached the store: a
/// cloud machine that ends takes them with it. They go into the outbox file
/// and onto the trunk, where the next machine to read the project sends
/// them.
fn carry_outbox(git: &Git, dir: &std::path::Path) {
    if memcli::sync_outbox() == Some(0) {
        return;
    }
    if !memcli::share_export() || git.quiet(&["diff", "--cached", "--quiet"]) {
        return;
    }
    let committed = git.quiet(&[
        "commit",
        "-q",
        "-m",
        "Carry the writes the store has not taken yet",
    ]);
    if !committed || push(dir, &format!("HEAD:{TRUNK}"), None).is_err() {
        warn(format!(
            "land: the writes the store has not taken are committed here but not on origin's {TRUNK}"
        ));
    }
}

/// The line a landed thread ends its report on: the next start line, done,
/// or parked while the project is paused.
fn next_line(project: &str) -> String {
    let paused = memcli::projects()
        .and_then(|rows| rows.into_iter().find(|row| row.name == project))
        .is_some_and(|row| row.paused.is_some());
    let next = memcli::roadmap(project).and_then(|text| go::next_milestone(&text));
    match next {
        None => format!("done: {project}'s roadmap is done"),
        Some(_) if paused => format!("parked: {project} is paused"),
        Some(_) => format!("next: {}", cloud::start_line(project)),
    }
}
