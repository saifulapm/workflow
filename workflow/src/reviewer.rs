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
//! The reader is a worker like any other: dispatched through the project's
//! backend (a `claude --bg` session or an amx pane, never print mode), so it
//! shows up in `claude agents` or `amx ls` and can be attached to while it
//! reads. It writes its answer to one file and ends; the gate reads the
//! verdict off that file and checks the tree it read is untouched.

use std::path::Path;

use crate::plan::Task;

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

/// The first `VERDICT: ship|fix` line in what the reviewer wrote, case
/// aside, with whatever markdown it wrapped the line in stripped off.
pub fn verdict(text: &str) -> Option<Verdict> {
    text.lines().find_map(|line| {
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

/// The reader's brief: the plan of record whole, so the rulings and the Done
/// line it holds the diff to are the ones the run holds it to; the task block
/// verbatim; the diff, or its stat past [`DIFF_CAP`]; and the contract --
/// one answer file, first line the verdict, nothing else written.
pub fn prompt(
    plan_text: &str,
    task: &Task,
    diff: &str,
    stat: &str,
    worktree: &Path,
    answer: &Path,
) -> String {
    let change = if diff.len() > DIFF_CAP {
        format!(
            "The diff is {} bytes, past what this brief carries, so this is its stat; \
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
You are in {wt}, which holds the tree with this diff applied; read files there
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

## How to answer

Write your whole answer, under 400 words, to exactly this file and then stop:

    Answer file: {answer}

Its first line is exactly one of:

    VERDICT: ship
    VERDICT: fix

then the findings, most severe first, or one line saying the diff is clean.
That file is the only thing you write. Do not edit, create or commit anything
in the tree, do not run its tests or builds, and do not ask questions: a
reading that changes the tree is void.

## The plan of record

{plan}

## The task

{block}
## The diff

{change}
",
        id = task.id,
        wt = worktree.display(),
        answer = answer.display(),
        plan = plan_text.trim_end(),
        block = task.block,
        change = change,
    )
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

#[cfg(test)]
mod tests {
    use super::*;

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
            Path::new("/runs/t3.review"),
        );
        for needle in [
            "# Review of task t3 before it merges",
            "Ruling 1. Config.",
            "Done: a fix verdict resets integration",
            "```diff\ndiff --git a/x b/x\n+fixed\n```",
            "You are in /state/wt/_integration",
            "Answer file: /runs/t3.review",
            "VERDICT: ship",
            "VERDICT: fix",
            "under 400 words",
            "preferences do\nnot block",
            "the only thing you write",
        ] {
            assert!(text.contains(needle), "the prompt lost {needle:?}:\n{text}");
        }
        assert!(!text.contains(" x | 1 +"), "a diff under the cap goes in whole, not as its stat");
    }

    #[test]
    fn a_diff_past_the_cap_goes_in_as_its_stat() {
        let big = "+".repeat(DIFF_CAP + 1);
        let text = prompt("# plan: p\n", &task(), &big, " x | 1 +\n", Path::new("/wt"), Path::new("/r"));
        assert!(text.contains(" x | 1 +"), "the stat stands in");
        assert!(text.contains("read the files themselves"), "and the reviewer is told to read");
        assert!(!text.contains(&big[..1000]), "the diff body is out");
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
