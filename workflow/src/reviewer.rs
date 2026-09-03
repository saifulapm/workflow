//! The reader at the merge gate (plan gate-reviewer).
//!
//! Verify proves what a test can reach; it never reads the diff against the
//! Done line the planner wrote, and two cold reviews of a project's merged
//! work found ten defects that had passed every Verify (mem #YJA08HKW). So a
//! task whose Verify is green on integration is read once more, by a model
//! in a clean context, before the merge is recorded. It sees the plan of
//! record, the task block and the diff, and answers `VERDICT: ship` or
//! `VERDICT: fix` with findings; `fix` takes the path a red Verify takes.
//!
//! The call goes through a template the way a worker's does, so a test can
//! stand a fake reviewer in for `claude -p`.

use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::backend::subst;
use crate::plan::Task;
use crate::sys;

/// The reviewer, in the integration worktree so its file reads see the
/// merged tree. Read-only tools and no session on disk: it is a reading, not
/// a session anyone resumes.
pub const CMD_DEFAULT: &str = "cd {worktree} && claude -p --model {model} --tools Read,Grep,Glob --no-session-persistence < {prompt} > {out} 2> {err}";

/// Past this the diff goes in as its `--stat`, and the reviewer reads the
/// files instead: a prompt that is mostly diff is a reading nobody does well.
pub const DIFF_CAP: usize = 200 * 1024;

/// Wall clock for one reading, in fractional minutes like the run's own
/// deadline.
pub const DEADLINE_MIN_DEFAULT: f64 = 15.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Ship,
    Fix,
}

/// The first `VERDICT: ship|fix` line in what the reviewer printed, case
/// aside, with whatever markdown it wrapped the line in stripped off.
pub fn verdict(stdout: &str) -> Option<Verdict> {
    stdout.lines().find_map(|line| {
        let line = line.trim().trim_start_matches(['*', '#', '-', '>', ' ']);
        let rest = line
            .get(..8)
            .filter(|head| head.eq_ignore_ascii_case("verdict:"))
            .map(|_| line[8..].trim_start_matches(['*', ' ']))?;
        let word: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphabetic())
            .collect();
        match word.to_ascii_lowercase().as_str() {
            "ship" => Some(Verdict::Ship),
            "fix" => Some(Verdict::Fix),
            _ => None,
        }
    })
}

/// What the reviewer reads: the plan of record whole, so the rulings and the
/// Done line it holds the diff to are the ones the run holds it to; the task
/// block verbatim; the diff, or its stat past [`DIFF_CAP`]; and the contract.
pub fn prompt(plan_text: &str, task: &Task, diff: &str, stat: &str, worktree: &Path) -> String {
    let change = if diff.len() > DIFF_CAP {
        format!(
            "The diff is {} bytes, past what this prompt carries, so this is its stat; \
             read the files themselves in the worktree.\n\n```\n{}\n```",
            diff.len(),
            stat.trim_end()
        )
    } else {
        format!("```diff\n{}\n```", diff.trim_end())
    };
    format!(
        "\
# Review of task {id} before it merges

You are a cold reviewer. You read the plan, the task block and the diff below,
and nothing else: not the worker's reasoning, not its commit messages' claims.
The worktree at {wt} holds the tree with this diff applied; read files there
when a judgement depends on code the diff does not show. The task's Verify
command has already passed on that tree, so a test is not what you are here
for -- what a test can reach is proved, and what only a reader can see is not.

Two lenses, answer both:

1. Reproduce a defect. A concrete input or state where this code does the
   wrong thing: a regression against what worked before, an event path the
   tests bypass, a value read from the wrong place, a crash on a platform the
   suite does not run on.
2. Spec compliance. Does the diff satisfy the task's Done line and the plan's
   rulings, all of it, nothing extra? Name each gap with the Done clause or
   ruling it misses. A ruling the worker read differently from what the plan
   plainly says is a gap.

A finding carries a file and line, the concrete failure, and the correct
behaviour. \"Consider extracting this\" is a preference, and preferences do
not block; correctness and requirement gaps do. Do not summarise the diff and
do not review style.

Answer in under 400 words. The first line of your answer is exactly one of:

    VERDICT: ship
    VERDICT: fix

then the findings, most severe first, or one line saying the diff is clean.

## The plan of record

{plan}

## The task

{block}
## The diff

{change}
",
        id = task.id,
        wt = worktree.display(),
        plan = plan_text.trim_end(),
        block = task.block,
        change = change,
    )
}

/// The reviewer's command from `WORKFLOW_REVIEW_CMD` or [`CMD_DEFAULT`].
pub fn command(model: &str, worktree: &Path, prompt: &Path, out: &Path, err: &Path) -> String {
    let template = match std::env::var("WORKFLOW_REVIEW_CMD") {
        Ok(v) if !v.is_empty() => v,
        _ => CMD_DEFAULT.to_string(),
    };
    command_from(&template, model, worktree, prompt, out, err)
}

/// The pure half: every placeholder becomes one shell word, so a project
/// whose path has a space in it is reviewed like any other.
pub fn command_from(
    template: &str,
    model: &str,
    worktree: &Path,
    prompt: &Path,
    out: &Path,
    err: &Path,
) -> String {
    let path = |p: &Path| p.to_string_lossy().to_string();
    let mut cmd = template.to_string();
    for (key, value) in [
        ("worktree", path(worktree)),
        ("prompt", path(prompt)),
        ("out", path(out)),
        ("err", path(err)),
        ("model", model.to_string()),
    ] {
        cmd = subst(&cmd, key, &value);
    }
    cmd
}

/// Seconds one reading may take: `WORKFLOW_REVIEW_DEADLINE_MIN`, fractional,
/// else [`DEADLINE_MIN_DEFAULT`].
pub fn deadline_s() -> i64 {
    deadline_from(std::env::var("WORKFLOW_REVIEW_DEADLINE_MIN").ok().as_deref())
}

fn deadline_from(value: Option<&str>) -> i64 {
    let minutes = value
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|m| *m > 0.0)
        .unwrap_or(DEADLINE_MIN_DEFAULT);
    ((minutes * 60.0) + 0.5).max(1.0) as i64
}

/// One reading: run the command, wait for it within the deadline, and read
/// the verdict off what it printed to `out`. `Err` is a reviewer that could
/// not be started, ran past the deadline, or printed no verdict -- never a
/// judgement on the code; the caller decides what a reading that did not
/// happen means.
pub fn review(model: &str, worktree: &Path, prompt: &Path, out: &Path, err: &Path) -> Result<Verdict, String> {
    run(&command(model, worktree, prompt, out, err), out, deadline_s())
}

pub fn run(cmd: &str, out: &Path, deadline_s: i64) -> Result<Verdict, String> {
    let _ = std::fs::write(out, "");
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|e| format!("the reviewer could not be started: {e}"))?;
    let until = sys::now() + deadline_s;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if sys::now() >= until => {
                sys::kill_group(&child.id().to_string(), "TERM");
                let _ = child.wait();
                return Err(format!(
                    "the review ran past its {deadline_s} second deadline and was stopped"
                ));
            }
            Ok(None) => sys::sleep(1.0),
            Err(e) => return Err(format!("lost track of the reviewer: {e}")),
        }
    };
    let printed = std::fs::read_to_string(out).unwrap_or_default();
    verdict(&printed).ok_or_else(|| match status.success() {
        true => "the review returned no verdict".to_string(),
        false => format!("the reviewer exited with {status} and no verdict"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::shq;
    use std::path::PathBuf;

    fn task() -> Task {
        Task {
            id: "t3".into(),
            title: "The gate step".into(),
            deps: vec!["t2".into()],
            files: Some("workflow/src/run.rs".into()),
            verify: Some("cargo test".into()),
            done: Some("a fix verdict resets integration".into()),
            read: None,
            uses: None,
            gives: None,
            pattern: None,
            checked: false,
            block: "- [ ] t3 The gate step  [after: t2]\n      Files: workflow/src/run.rs\n      Verify: cargo test\n      Done: a fix verdict resets integration\n".into(),
        }
    }

    #[test]
    fn the_verdict_is_the_first_verdict_line_however_it_is_dressed() {
        assert_eq!(verdict("VERDICT: ship\n\nThe diff is clean."), Some(Verdict::Ship));
        assert_eq!(verdict("Reading the diff...\nverdict: FIX\n1. run.rs:10"), Some(Verdict::Fix));
        assert_eq!(verdict("**VERDICT: fix**\n- run.rs:10"), Some(Verdict::Fix));
        assert_eq!(verdict("## Verdict: Ship"), Some(Verdict::Ship));
        assert_eq!(verdict("VERDICT: fix\nVERDICT: ship"), Some(Verdict::Fix), "the first one counts");
        assert_eq!(verdict("VERDICT: maybe\nVERDICT: ship"), Some(Verdict::Ship), "an unknown word is skipped");
        assert_eq!(verdict("The verdict is that it ships."), None);
        assert_eq!(verdict("VERDICTS: ship"), None);
        assert_eq!(verdict(""), None);
    }

    #[test]
    fn the_prompt_carries_the_plan_the_block_the_diff_and_the_contract() {
        let text = prompt(
            "# plan: gate-reviewer\n\n## Spec\n\nRuling 1. Config.\n",
            &task(),
            "diff --git a/x b/x\n+fixed\n",
            " x | 1 +\n",
            Path::new("/state/wt/_integration"),
        );
        for needle in [
            "# Review of task t3 before it merges",
            "Ruling 1. Config.",
            "Done: a fix verdict resets integration",
            "```diff\ndiff --git a/x b/x\n+fixed\n```",
            "/state/wt/_integration",
            "VERDICT: ship",
            "VERDICT: fix",
            "under 400 words",
            "preferences do\nnot block",
        ] {
            assert!(text.contains(needle), "the prompt lost {needle:?}:\n{text}");
        }
        assert!(!text.contains(" x | 1 +"), "a diff under the cap goes in whole, not as its stat");
    }

    #[test]
    fn a_diff_past_the_cap_goes_in_as_its_stat() {
        let big = "+".repeat(DIFF_CAP + 1);
        let text = prompt("# plan: p\n", &task(), &big, " x | 1 +\n", Path::new("/wt"));
        assert!(text.contains(" x | 1 +"), "the stat stands in");
        assert!(text.contains("read the files themselves"), "and the reviewer is told to read");
        assert!(!text.contains(&big[..1000]), "the diff body is out");
    }

    #[test]
    fn every_placeholder_is_one_shell_word() {
        let cmd = command_from(
            CMD_DEFAULT,
            "fable",
            Path::new("/state/my project/_integration"),
            Path::new("/runs/t3.review-prompt"),
            Path::new("/runs/t3.review"),
            Path::new("/runs/t3.review-err"),
        );
        assert!(cmd.starts_with("cd '/state/my project/_integration' && claude -p --model 'fable'"));
        assert!(cmd.contains("< '/runs/t3.review-prompt' > '/runs/t3.review' 2> '/runs/t3.review-err'"));
        assert!(cmd.contains("--tools Read,Grep,Glob"), "read-only tools: {cmd}");
        assert!(!cmd.contains('{'), "a placeholder was left behind: {cmd}");
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wf-reviewer-{}-{name}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir.join("out")
    }

    #[test]
    fn a_reading_returns_what_the_reviewer_printed_or_says_why_not() {
        let out = scratch("fix");
        let cmd = format!("printf 'Some preamble\\nVERDICT: fix\\n1. x.rs:3\\n' > {}", shq(&out.to_string_lossy()));
        assert_eq!(run(&cmd, &out, 30), Ok(Verdict::Fix));

        let out = scratch("none");
        let cmd = format!("printf 'I could not decide.\\n' > {}", shq(&out.to_string_lossy()));
        assert_eq!(run(&cmd, &out, 30), Err("the review returned no verdict".into()));

        let out = scratch("crash");
        assert_eq!(
            run("exit 3", &out, 30),
            Err("the reviewer exited with exit status: 3 and no verdict".into())
        );

        let out = scratch("slow");
        let started = sys::now();
        let err = run("sleep 30; printf 'VERDICT: ship'", &out, 1).unwrap_err();
        assert!(err.contains("ran past its 1 second deadline"), "{err}");
        assert!(sys::now() - started < 10, "the deadline was enforced, not waited out");
    }

    #[test]
    fn the_deadline_is_fractional_minutes_with_a_floor_of_one_second() {
        assert_eq!(deadline_from(None), 900);
        assert_eq!(deadline_from(Some("0.05")), 3, "three seconds, the way a test injects one");
        assert_eq!(deadline_from(Some("0.001")), 1, "never zero");
        assert_eq!(deadline_from(Some("nonsense")), 900);
        assert_eq!(deadline_from(Some("-2")), 900);
    }
}
