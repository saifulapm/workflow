//! `workflow read` -- the merge gate's own reader, started by hand over a
//! working tree or a range, for the one-shot lane and the review skill (see
//! wiki:merge-gate). No run, no worktree of its own: one prompt, one reader,
//! one verdict file, and the exit code says what it found.

use crate::backend::{Dispatch, Handle};
use crate::gitcmd::Git;
use crate::plan::Task;
use crate::reviewer::{self, Verdict};
use crate::{exit, memcli, paths, repo, run, sys, verify, warn};

/// Past this many bytes an untracked file is named but not inlined: the same
/// reasoning as the diff cap, on a much smaller document.
const UNTRACKED_CAP: usize = 24 * 1024;

const DEFAULT_REQUIREMENT: &str =
    "the change is correct, complete and changes nothing it was not asked to";

/// Nobody is named to read: `WORKFLOW_REVIEW_MODEL` names one over anything
/// recorded, even empty; absent, the project's own `review-model` stands,
/// `none` or unset there too meaning the same as nobody.
const NO_READER: i32 = 2;
/// The reading ended -- by its own hand, by the deadline, or because the
/// dispatch never started -- without a verdict to answer with.
const NO_VERDICT: i32 = 3;

fn model() -> Option<String> {
    match std::env::var("WORKFLOW_REVIEW_MODEL") {
        Ok(v) => {
            let v = v.trim();
            (!v.is_empty() && !v.eq_ignore_ascii_case("none")).then(|| v.to_string())
        }
        Err(_) => memcli::project_review_model(),
    }
}

/// The reasoning dial: `WORKFLOW_REVIEW_EFFORT` over the project's own
/// `review-effort`, `none` in either meaning no dial at all -- the same drop
/// `model` above already applies.
fn effort() -> Option<String> {
    match std::env::var("WORKFLOW_REVIEW_EFFORT") {
        Ok(v) => {
            let v = v.trim();
            (!v.is_empty() && !v.eq_ignore_ascii_case("none")).then(|| v.to_string())
        }
        Err(_) => memcli::project_review_effort().filter(|v| !v.eq_ignore_ascii_case("none")),
    }
}

/// `git status --porcelain --untracked-files=all`'s paths, each under its own
/// heading with its contents inlined past [`UNTRACKED_CAP`], or a note that
/// it went past the cap: the diff alone never shows a brand-new file
/// (review-3 F-12). The default `-unormal` collapses a new directory into one
/// entry that is not a file `fs::read` can open, so every untracked path is
/// asked for by name.
fn untracked_section(git: &Git) -> String {
    let listed = git
        .out(&["status", "--porcelain", "--untracked-files=all"])
        .unwrap_or_default();
    let paths: Vec<&str> = listed
        .lines()
        .filter_map(|line| line.strip_prefix("?? "))
        .collect();
    if paths.is_empty() {
        return String::new();
    }
    let mut out = String::from("\n\nUntracked files\n");
    for path in paths {
        out.push_str(&format!("\n{path}:\n"));
        match std::fs::read(path) {
            Ok(bytes) if bytes.len() <= UNTRACKED_CAP => {
                out.push_str("```\n");
                out.push_str(&String::from_utf8_lossy(&bytes));
                out.push_str("\n```\n");
            }
            Ok(bytes) => out.push_str(&format!(
                "{} bytes, past what this brief carries inline.\n",
                bytes.len()
            )),
            Err(e) => out.push_str(&format!("cannot be read ({e}).\n")),
        }
    }
    out
}

/// What the diff is held to: `--against`, a `wiki:<slug>` on it resolved
/// through mem, else the project's own plan, else a generic requirement.
fn requirement(against: Option<&str>) -> String {
    let named = against.and_then(|text| match text.strip_prefix("wiki:") {
        Some(slug) => memcli::wiki_page(slug),
        None => Some(text.to_string()),
    });
    named
        .or_else(memcli::plan)
        .unwrap_or_else(|| DEFAULT_REQUIREMENT.to_string())
}

/// The diff to read and its stat, cold. `Git::out` cannot tell a failed
/// command from an empty one, so a range is asked for with `capture` and a
/// non-zero status is refused by name rather than read as an empty, clean
/// diff.
fn diff_and_stat(git: &Git, range: Option<&str>) -> Result<(String, String), String> {
    match range {
        Some(r) => {
            let out = git.capture(&["diff", r]);
            if !out.ok {
                let err = String::from_utf8_lossy(&out.stderr);
                let line = err
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("git diff failed")
                    .to_string();
                return Err(line);
            }
            let diff = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let stat = git.out(&["diff", "--stat", r]).unwrap_or_default();
            Ok((diff, stat))
        }
        None => {
            let mut diff = git.out(&["diff", "HEAD"]).unwrap_or_default();
            diff.push_str(&untracked_section(git));
            let stat = git.out(&["diff", "--stat", "HEAD"]).unwrap_or_default();
            Ok((diff, stat))
        }
    }
}

pub fn cmd_read(range: Option<&str>, against: Option<&str>) -> i32 {
    if !Git::here().inside_worktree() {
        warn("read: not inside a git work tree");
        return exit::USAGE;
    }
    let Some((git, top)) = repo::goto_toplevel() else {
        warn("read: cannot resolve the repository toplevel");
        return exit::USAGE;
    };

    let Some(model) = model() else {
        warn("read: nobody is named to read this diff.");
        warn(
            "name one with `mem project set review-model <model>`, or point WORKFLOW_REVIEW_MODEL at one for this call.",
        );
        return NO_READER;
    };

    let (diff, stat) = match diff_and_stat(&git, range) {
        Ok(v) => v,
        Err(line) => {
            warn(format!("read: {line}"));
            return NO_VERDICT;
        }
    };
    if diff.trim().is_empty() {
        warn("read: nothing to read -- no diff and no untracked files");
        return NO_VERDICT;
    }

    let requirement = requirement(against);
    let title = top
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let task = Task {
        id: "read".into(),
        title: title.clone(),
        deps: Vec::new(),
        files: None,
        verify: None,
        done: Some(requirement.clone()),
        read: None,
        uses: None,
        gives: None,
        pattern: None,
        checked: false,
        block: format!("- [ ] read {title}\n      Done: {requirement}\n"),
    };

    let project = memcli::project_current();
    let project_dir = project
        .as_ref()
        .map(|p| p.dir_name())
        .unwrap_or_else(|| paths::path_slug(&top));
    let backend = run::backend_for();
    let mint = backend.mint_session();
    // Keyed by the second and a minted suffix: two calls starting in the same
    // second in the same project each get their own directory rather than
    // wiping each other's answer and pidfile mid-poll.
    let dir = paths::runs_root()
        .join(&project_dir)
        .join("_read")
        .join(format!("{}-{mint}", sys::now()));
    if let Err(e) = std::fs::create_dir_all(&dir) {
        warn(format!("read: cannot make {} ({e})", dir.display()));
        return exit::USAGE;
    }

    let gate = verify::detect_verifiers(&top, project.as_ref())
        .into_iter()
        .map(|v| format!("{}: {}", v.label, v.cmd))
        .collect::<Vec<_>>()
        .join("\n");

    let prompt_path = dir.join("read.review-prompt");
    let answer = dir.join("read.review");
    let pidfile = dir.join("read.review-pid");
    let _ = std::fs::write(
        &prompt_path,
        reviewer::prompt("", &task, &diff, &stat, &top, &answer, &[], &gate, &[]),
    );

    let d = Dispatch {
        task: "read-review".into(),
        worktree: top.clone(),
        brief: prompt_path,
        out: dir.join("read.review-out"),
        err: dir.join("read.review-err"),
        pidfile,
        status: dir.join("read.review-status"),
        rundir: dir.clone(),
        session: mint,
        model,
        effort: effort(),
        turns: match std::env::var("WORKFLOW_MAX_TURNS") {
            Ok(v) if !v.is_empty() => v,
            _ => "120".into(),
        },
        env: Vec::new(),
    };

    // The reader is dispatched into the caller's own live working tree, the
    // one holding the diff it is reading: a before-and-after snapshot is
    // enough to warn if it left something behind (wiki:merge-gate step 6
    // voids a reading that does this at the gate).
    let before_tree = git
        .out(&["status", "--porcelain", "--untracked-files=all"])
        .unwrap_or_default();

    let dispatched = backend.dispatch(&d);
    // No handle is a launch the backend refused -- amx at its cap, or a tmux
    // it cannot reach -- and there is no reading to wait on. What it said is
    // in the err file, so this fails on that line rather than spinning out
    // the full deadline on a session that never came up (run.rs does the
    // same at its own dispatch).
    if dispatched.is_empty() {
        let said = std::fs::read_to_string(&d.err).unwrap_or_default();
        let line = said
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("nothing on stderr");
        warn(format!("read: the launch was refused: {line}"));
        return NO_VERDICT;
    }
    let h = Handle {
        session: dispatched,
        pidfile: d.pidfile.clone(),
        worktree: top.clone(),
    };

    let deadline_s = reviewer::deadline_s();
    let grace_s = (deadline_s / 2).clamp(1, 30);
    let started = sys::now();
    let mut timed_out = false;
    while backend.alive(&h) {
        if sys::now() - started >= deadline_s {
            timed_out = true;
            break;
        }
        sys::sleep(1.0);
    }
    backend.stop(&h, grace_s);

    let after_tree = git
        .out(&["status", "--porcelain", "--untracked-files=all"])
        .unwrap_or_default();
    if after_tree != before_tree {
        warn("read: the reading left the working tree changed");
    }

    if timed_out {
        warn(format!(
            "read: the reading ran past its {deadline_s} second deadline and was stopped -- read {}",
            answer.display()
        ));
        return NO_VERDICT;
    }

    let text = std::fs::read_to_string(&answer).unwrap_or_default();
    match reviewer::verdict(&text) {
        Some(Verdict::Ship) => {
            print!("{text}");
            exit::OK
        }
        Some(Verdict::Fix) => {
            print!("{text}");
            exit::FAILED
        }
        None => {
            warn(format!("read: no verdict -- read {}", answer.display()));
            NO_VERDICT
        }
    }
}
