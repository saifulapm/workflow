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

/// `git status --porcelain`'s untracked paths, each under its own heading with
/// its contents inlined past [`UNTRACKED_CAP`], or a note that it went past
/// the cap: the diff alone never shows a brand-new file (review-3 F-12).
fn untracked_section(git: &Git) -> String {
    let listed = git.out(&["status", "--porcelain"]).unwrap_or_default();
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

pub fn cmd_read(range: Option<&str>, against: Option<&str>) -> i32 {
    if !Git::here().inside_worktree() {
        warn("read: not inside a git work tree");
        return exit::FAILED;
    }
    let Some((git, top)) = repo::goto_toplevel() else {
        warn("read: cannot resolve the repository toplevel");
        return exit::FAILED;
    };

    let Some(model) = model() else {
        warn("read: nobody is named to read this diff.");
        warn(
            "name one with `mem project set review-model <model>`, or point WORKFLOW_REVIEW_MODEL at one for this call.",
        );
        return NO_READER;
    };

    let (diff, stat) = match range {
        Some(r) => (
            git.out(&["diff", r]).unwrap_or_default(),
            git.out(&["diff", "--stat", r]).unwrap_or_default(),
        ),
        None => {
            let mut diff = git.out(&["diff", "HEAD"]).unwrap_or_default();
            diff.push_str(&untracked_section(&git));
            (
                diff,
                git.out(&["diff", "--stat", "HEAD"]).unwrap_or_default(),
            )
        }
    };

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
    let dir = paths::runs_root()
        .join(&project_dir)
        .join("_read")
        .join(sys::now().to_string());
    if let Err(e) = std::fs::create_dir_all(&dir) {
        warn(format!("read: cannot make {} ({e})", dir.display()));
        return exit::FAILED;
    }

    let gate = verify::detect_verifiers(&top, project.as_ref())
        .into_iter()
        .map(|v| format!("{}: {}", v.label, v.cmd))
        .collect::<Vec<_>>()
        .join("\n");

    let prompt_path = dir.join("read.review-prompt");
    let answer = dir.join("read.review");
    let pidfile = dir.join("read.review-pid");
    // The directory is keyed by the second, and two calls inside the same
    // one land in it together: drop whatever the last reading left before
    // this one starts, the way `read_start` does for a task's own review.
    let _ = std::fs::remove_file(&answer);
    let _ = std::fs::remove_file(&pidfile);
    let _ = std::fs::write(
        &prompt_path,
        reviewer::prompt("", &task, &diff, &stat, &top, &answer, &[], &gate, &[]),
    );

    let backend = run::backend_for();
    let d = Dispatch {
        task: "read-review".into(),
        worktree: top.clone(),
        brief: prompt_path,
        out: dir.join("read.review-out"),
        err: dir.join("read.review-err"),
        pidfile,
        status: dir.join("read.review-status"),
        rundir: dir.clone(),
        session: backend.mint_session(),
        model,
        effort: std::env::var("WORKFLOW_REVIEW_EFFORT")
            .ok()
            .filter(|v| !v.is_empty()),
        turns: match std::env::var("WORKFLOW_MAX_TURNS") {
            Ok(v) if !v.is_empty() => v,
            _ => "120".into(),
        },
        env: Vec::new(),
    };
    let h = Handle {
        session: backend.dispatch(&d),
        pidfile: d.pidfile.clone(),
        worktree: top.clone(),
    };

    let deadline_s = reviewer::deadline_s();
    let grace_s = (deadline_s / 2).clamp(1, 30);
    let started = sys::now();
    while backend.alive(&h) {
        if sys::now() - started >= deadline_s {
            backend.stop(&h, grace_s);
            warn(format!(
                "read: the reading ran past its {deadline_s} second deadline and was stopped -- read {}",
                answer.display()
            ));
            return NO_VERDICT;
        }
        sys::sleep(1.0);
    }
    backend.stop(&h, grace_s);

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
