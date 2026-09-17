//! `workflow read` -- the merge gate's own reader, started by hand over a
//! working tree or a range, for the one-shot lane and the review skill (see
//! wiki:merge-gate). No run, no worktree of its own: one prompt, one reader,
//! one verdict file, and the exit code says what it found.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use crate::backend::{Dispatch, Handle};
use crate::gitcmd::{self, Git};
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

/// Claude Code's own scratch inside the tree: a sub-agent's memory under
/// `.claude/agent-memory/`, the permissions a session allowed in
/// `.claude/settings.local.json`. The gate never counts it as a task's write
/// (ownership.rs `harness_scratch`) and a reading must not either -- the
/// reader is not here to judge the harness, and the reader's own session
/// writes scratch into the very tree it is reading, which would trip the
/// before-and-after check in [`cmd_read`] on a change nobody made. Only the
/// untracked record is waved through, as at the gate: a tracked file under
/// `.claude/` that the change touches is still its write.
fn harness_scratch(field: &[u8]) -> bool {
    let Some(path) = field.strip_prefix(b"?? ") else {
        return false;
    };
    path.starts_with(b".claude/") || path.windows(9).any(|w| w == b"/.claude/")
}

/// The working tree's status, as fields git wrote them.
///
/// `-z` because it is the only output that does not quote: a path holding a
/// space or a byte outside ASCII comes back from `--porcelain` alone wrapped
/// in double quotes, and a quoted path is not one `fs::read` can open
/// (ownership.rs says the same, in as many words). `-uall` because the
/// default collapses a brand-new directory into one entry that is not a file
/// either.
///
/// A rename is two fields rather than one record, which is enough here: this
/// is read for the `??` entries and for a before-and-after equality check,
/// and neither cares where one record ends.
fn status_fields(git: &Git) -> Vec<Vec<u8>> {
    gitcmd::nul_fields(&git.bytes(&["status", "--porcelain", "-uall", "-z"]))
        .into_iter()
        .filter(|f| !harness_scratch(f))
        .collect()
}

/// A fence longer than the longest backtick run the contents hold, so an
/// untracked markdown file cannot close its own block early and read as prose
/// beside the question.
fn fence_for(text: &str) -> String {
    let mut longest = 0usize;
    let mut run = 0usize;
    for c in text.chars() {
        if c == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    "`".repeat(longest.max(2) + 1)
}

/// The untracked paths, each under its own heading with its contents inlined
/// under [`UNTRACKED_CAP`], or a note saying why not: the diff alone never
/// shows a brand-new file (review-3 F-12). Bytes that are not UTF-8 are named
/// and not inlined -- a binary fixture rendered through `from_utf8_lossy` is
/// a screenful of replacement characters that tells the reader nothing.
fn untracked_section(fields: &[Vec<u8>]) -> String {
    let paths: Vec<&[u8]> = fields
        .iter()
        .filter_map(|f| f.strip_prefix(b"?? ".as_slice()))
        .collect();
    if paths.is_empty() {
        return String::new();
    }
    let mut out = String::from("\n\nUntracked files\n");
    for path in paths {
        out.push_str(&format!("\n{}:\n", String::from_utf8_lossy(path)));
        match std::fs::read(Path::new(OsStr::from_bytes(path))) {
            Ok(bytes) if bytes.len() > UNTRACKED_CAP => out.push_str(&format!(
                "{} bytes, past what this brief carries inline.\n",
                bytes.len()
            )),
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(text) => {
                    let fence = fence_for(&text);
                    out.push_str(&format!(
                        "{fence}\n{}\n{fence}\n",
                        text.trim_end_matches('\n')
                    ));
                }
                Err(e) => out.push_str(&format!("binary, {} bytes.\n", e.as_bytes().len())),
            },
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

/// The first thing something said, for a refusal that fits on one line.
fn first_line(text: &str, fallback: &str) -> String {
    text.lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or(fallback)
        .to_string()
}

/// The diff to read and its stat, cold, with the untracked files beside it
/// when the diff is the working tree's own.
///
/// `Git::out` cannot tell a failed command from an empty one, so the diff is
/// asked for with `capture` and a non-zero status is refused by name. That is
/// not only about a range nobody can resolve: `git diff HEAD` exits 128 on an
/// unborn HEAD, and reading that as an empty diff would hand the reader the
/// untracked files alone while every staged file stayed invisible.
fn diff_and_stat(
    git: &Git,
    range: Option<&str>,
    fields: &[Vec<u8>],
) -> Result<(String, String), String> {
    let rev = range.unwrap_or("HEAD");
    let out = git.capture(&["diff", rev]);
    if !out.ok {
        return Err(first_line(
            &String::from_utf8_lossy(&out.stderr),
            "git diff failed",
        ));
    }
    let mut diff = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let stat = git.out(&["diff", "--stat", rev]).unwrap_or_default();
    if range.is_none() {
        diff.push_str(&untracked_section(fields));
    }
    Ok((diff, stat))
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

    // The one listing behind both the untracked section and the snapshot the
    // reading is judged against below.
    let before_tree = status_fields(&git);

    let (diff, stat) = match diff_and_stat(&git, range, &before_tree) {
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
    // The requirement is both the Done line (ruling 1) and the plan of record:
    // there is no plan document behind a read, and `reviewer::prompt` opens on
    // an empty "## The plan of record" heading otherwise, while the reader is
    // asked to judge the plan's rulings.
    let _ = std::fs::write(
        &prompt_path,
        reviewer::prompt(
            &requirement,
            &task,
            &diff,
            &stat,
            &top,
            &answer,
            &[],
            &gate,
            &[],
        ),
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
        role: "reader".into(),
        model,
        effort: effort(),
        turns: match std::env::var("WORKFLOW_MAX_TURNS") {
            Ok(v) if !v.is_empty() => v,
            _ => "120".into(),
        },
        env: Vec::new(),
    };

    // The reader is dispatched into the caller's own live working tree, the
    // one holding the diff it is reading: the snapshot taken above and the
    // one below are enough to warn if it left something behind
    // (wiki:merge-gate step 6 voids a reading that does this at the gate).
    let dispatched = backend.dispatch(&d);
    // No handle is a launch the backend refused -- amx at its cap, or a tmux
    // it cannot reach -- and there is no reading to wait on. What it said is
    // in the err file, so this fails on that line rather than spinning out
    // the full deadline on a session that never came up (run.rs does the
    // same at its own dispatch).
    if dispatched.is_empty() {
        let said = std::fs::read_to_string(&d.err).unwrap_or_default();
        let line = first_line(&said, "nothing on stderr");
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

    if status_fields(&git) != before_tree {
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
