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
//! The reader is a worker like any other: an amx agent in a pane, never print
//! mode, so it shows up in `amx ls` and can be attached to while it reads. It writes its answer to one file and ends; the gate reads the
//! verdict off that file and checks the tree it read is untouched.

use std::path::Path;

use crate::brief;
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

/// The findings a reading marked `later`: true, worth a line in a later
/// plan, and not worth a round of this one. Each is the line it was written
/// on, tag stripped, so a follow-up record reads as the finding itself.
pub fn later(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim().trim_start_matches(['*', '-', '>', ' ']);
            let rest = line.strip_prefix("[later]")?;
            let rest = rest.trim();
            (!rest.is_empty()).then(|| rest.to_string())
        })
        .collect()
}

/// Every earlier reading of this task, so a reader sent back after a fix
/// does not spend its whole reading on ground the first reading already
/// covered (ruling 3): each `<task>.review.<n>` verbatim under its own
/// heading, then the instruction to verdict each earlier finding before
/// reading the diff fresh for anything else.
fn earlier_section(earlier: &[String]) -> String {
    if earlier.is_empty() {
        return String::new();
    }
    let mut out = String::from("## Earlier readings of this task\n\n");
    for (i, text) in earlier.iter().enumerate() {
        out.push_str(&format!("### Reading {}\n\n", i + 1));
        out.push_str(text.trim_end());
        out.push_str("\n\n");
    }
    out.push_str(
        "The worker was sent back with these and told to fix every instance of \
         each finding's class. First verdict each earlier finding: addressed or \
         not, with file and line. Then read the whole diff again for anything \
         else, and name every instance of a class you find, not the first: each \
         reading costs a round.\n\n",
    );
    if earlier.len() >= 2 {
        out.push_str(
            "This is the third reading. Two rounds of fixes have gone into this diff \
             and a third is the orchestrator's call, not yours: only an earlier \
             finding still not addressed, or a defect the fixes themselves \
             introduced, is `[blocks]` here. Anything else you find now is true \
             and `[later]`, however real -- the lines it is on were in front of the \
             first two readings.\n\n",
        );
    }
    out
}

/// The reader's brief: the plan of record whole, so the rulings and the Done
/// line it holds the diff to are the ones the run holds it to; the wiki
/// pages the task's Read: named, verbatim, under the same heading the
/// worker's brief uses (ruling 1 of m1-wiki-first); every earlier reading of
/// this task, if any (ruling 3); the task block verbatim; the diff, or its
/// stat past [`DIFF_CAP`]; the gate's own commands, so a red gate is never
/// mistaken for a finding; and the contract -- one answer file, first line
/// the verdict, nothing else written.
#[allow(clippy::too_many_arguments)]
pub fn prompt(
    plan_text: &str,
    task: &Task,
    diff: &str,
    stat: &str,
    worktree: &Path,
    answer: &Path,
    pages: &[(String, Option<String>)],
    gate: &str,
    earlier: &[String],
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
    let gate_section = if gate.trim().is_empty() {
        "The gate runs no checks of its own here: no verifier is detected in \
         this tree, so a compile error or a failing test is yours to name."
            .to_string()
    } else {
        format!(
            "While you read, the gate runs the project's own checks on this same tree:\n\n\
             {}\n\n\
             A compile error, a type error, a lint or a failing test is the gate's to\n\
             find, and a red gate voids this reading, so never report one and never run\n\
             them. You are here for what those cannot reach.",
            gate.trim_end()
        )
    };
    format!(
        "\
# Review of task {id} before it merges

You are a cold reviewer. You read the plan, the task block and the diff below,
and nothing else: not the worker's reasoning, not its commit messages' claims.
You are in {wt}, which holds the tree with this diff applied; read files there
when a judgement depends on code the diff does not show. What to look for and
how to answer come after the material, at the end.

## The plan of record

{plan}

{pages}{earlier}## The task

{block}
## The diff

{change}

## What to look for

{gate_section}

Two lenses, answer both:

1. Reproduce a defect. A concrete input or state where this code does the
   wrong thing: a regression against what worked before, an event path the
   tests bypass, a value read from the wrong place, a crash on a platform the
   suite does not run on.
2. Spec compliance. Does the diff satisfy the task's Done line and the plan's
   rulings, all of it, nothing extra? Name each gap with the Done clause or
   ruling it misses. A ruling the worker read differently from what the plan
   plainly says is a gap.

Report every defect and gap you find, the ones you are not sure of included,
marked as such: nothing is held back here, and a real one left unsaid costs a
whole round. Each finding opens with one of two tags. `[blocks]` is a defect
or a gap the merge must not land with: wrong output, a crash, data another
tenant can reach, a Done clause or ruling missed. `[later]` is true and worth
fixing and not worth a round of this task -- a cap checked after the
allocation, a bound a bind parameter could use, a surrogate a decoder folds
-- and rides out of a ship verdict as a follow-up for a later plan. A finding
carries a file and line, the concrete failure, and the correct behaviour, in
a few lines each:

    - [blocks] src/cart.rs:40 -- rounds each line to the cent before
      summing, so a basket of three 0.335 items totals 1.02 where the Done
      line wants 1.01; sum in millicents and round once.
    - [later] src/cart.rs:88 -- the basket is cloned once per line; one
      clone before the loop does.

\"Consider extracting this\" is a preference, and preferences are not
findings; correctness and requirement gaps are. Do not summarise the diff and
do not review style.

## How to answer

Write your whole answer to exactly this file and then stop:

    Answer file: {answer}

Its first line is exactly one of:

    VERDICT: ship
    VERDICT: fix

then the findings, `[blocks]` before `[later]`, or one line saying the diff is
clean. `fix` only when at least one finding is `[blocks]`: a reading whose
findings are all `[later]` ships, and each of them is kept.
That file is the only thing you write. Do not edit, create or commit anything
in the tree, do not run its tests or builds, and do not ask questions: a
reading that changes the tree is void.
",
        id = task.id,
        wt = worktree.display(),
        gate_section = gate_section,
        answer = answer.display(),
        plan = plan_text.trim_end(),
        pages = brief::pages_section(&task.id, pages),
        earlier = earlier_section(earlier),
        block = task.block,
        change = change,
    )
}

/// Phrases a provider itself writes when it, not the diff, is the reason a
/// reading ended -- a usage limit, a rate limit, an outage, a session that
/// was never logged in. Case aside, since nothing pins how a provider
/// capitalises its own message.
const PROVIDER_LIMIT_MARKERS: [&str; 5] = [
    "reached your",
    "usage limit",
    "rate limit",
    "overloaded",
    "not logged in",
];

/// The line, if any, where a reader's last words read as a provider's own
/// refusal rather than a judgement on the diff. A match here is not a second
/// reading's kind of problem: the run says so and stops.
pub fn provider_limit(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let lower = line.to_ascii_lowercase();
        PROVIDER_LIMIT_MARKERS
            .iter()
            .any(|marker| lower.contains(marker))
            .then(|| line.trim().to_string())
    })
}

/// Seconds one reading may take: `WORKFLOW_REVIEW_DEADLINE_MIN`, fractional,
/// else [`DEADLINE_MIN_DEFAULT`].
pub fn deadline_s() -> i64 {
    deadline_from(
        std::env::var("WORKFLOW_REVIEW_DEADLINE_MIN")
            .ok()
            .as_deref(),
    )
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
    fn later_findings_are_the_lines_so_tagged_with_the_tag_stripped() {
        let text = "VERDICT: ship\n\
                    - [later] src/cart.rs:88 -- one clone before the loop does.\n\
                    - [blocks] src/cart.rs:40 -- rounds too early.\n\
                    * [later]  src/cart.rs:90 -- a bound on the bind parameter.\n\
                    - [later]\n";
        assert_eq!(
            later(text),
            [
                "src/cart.rs:88 -- one clone before the loop does.",
                "src/cart.rs:90 -- a bound on the bind parameter.",
            ]
        );
        assert!(later("VERDICT: ship\nThe diff is clean.").is_empty());
    }

    #[test]
    fn the_third_reading_is_told_what_may_still_block() {
        let two = [
            "VERDICT: fix\n- [blocks] a".to_string(),
            "VERDICT: fix\n- [blocks] b".to_string(),
        ];
        let text = earlier_section(&two);
        assert!(text.contains("This is the third reading"), "{text}");
        assert!(text.contains("### Reading 2"), "{text}");
        let one = &two[..1];
        assert!(!earlier_section(one).contains("third reading"));
        assert!(earlier_section(&[]).is_empty());
    }

    #[test]
    fn the_verdict_is_the_first_verdict_line_however_it_is_dressed() {
        assert_eq!(
            verdict("VERDICT: ship\n\nThe diff is clean."),
            Some(Verdict::Ship)
        );
        assert_eq!(
            verdict("Reading the diff...\nverdict: FIX\n1. run.rs:10"),
            Some(Verdict::Fix)
        );
        assert_eq!(verdict("**VERDICT: fix**\n- run.rs:10"), Some(Verdict::Fix));
        assert_eq!(verdict("## Verdict: Ship"), Some(Verdict::Ship));
        assert_eq!(
            verdict("VERDICT: fix\nVERDICT: ship"),
            Some(Verdict::Fix),
            "the first one counts"
        );
        assert_eq!(
            verdict("VERDICT: maybe\nVERDICT: ship"),
            Some(Verdict::Ship),
            "an unknown word is skipped"
        );
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
            &[],
            "rust: cargo test",
            &[],
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
            "Report every defect and gap you find, the ones you are not sure of included",
            "preferences are not",
            "[blocks] src/cart.rs:40 -- rounds each line to the cent",
            "[later] src/cart.rs:88",
            "`fix` only when at least one finding is `[blocks]`",
            "That file is the only thing you write",
        ] {
            assert!(text.contains(needle), "the prompt lost {needle:?}:\n{text}");
        }
        assert!(
            !text.contains(" x | 1 +"),
            "a diff under the cap goes in whole, not as its stat"
        );
        // The material comes first and the instructions last: a reading of
        // a long prompt is better when the question sits under the documents.
        let diff = text.find("## The diff").unwrap();
        let lenses = text.find("## What to look for").unwrap();
        let answer = text.find("## How to answer").unwrap();
        assert!(diff < lenses && lenses < answer, "{text}");
    }

    /// The gate's own commands are named and ruled out of the reading, so a
    /// compile error or a failing test is never mistaken for a finding
    /// (ruling 2).
    #[test]
    fn the_prompt_names_the_gates_commands_and_rules_out_their_failures() {
        let text = prompt(
            "# plan: gate-reviewer\n",
            &task(),
            "diff --git a/x b/x\n+fixed\n",
            " x | 1 +\n",
            Path::new("/state/wt/_integration"),
            Path::new("/runs/t3.review"),
            &[],
            "php: ./bin/php artisan test",
            &[],
        );
        assert!(
            text.contains(
                "While you read, the gate runs the project's own checks on this same tree:"
            ),
            "{text}"
        );
        assert!(text.contains("php: ./bin/php artisan test"), "{text}");
        assert!(
            text.contains(
                "A compile error, a type error, a lint or a failing test is the gate's to"
            ),
            "{text}"
        );
        assert!(
            text.contains("never report one and never run\nthem"),
            "{text}"
        );
        assert!(
            !text.contains("The task's Verify command has already passed"),
            "the gate's suite replaces the worker's own Verify as what a reader defers to: {text}"
        );
    }

    /// With no verifier detected in the tree, `gate` is the empty string and
    /// the prompt must not tell the reader a check runs that never does
    /// (ruling 2, amended): real breakage there is the reader's to name.
    #[test]
    fn no_verifier_detected_leaves_breakage_the_readers_to_name() {
        let text = prompt(
            "# plan: gate-reviewer\n",
            &task(),
            "diff --git a/x b/x\n+fixed\n",
            " x | 1 +\n",
            Path::new("/state/wt/_integration"),
            Path::new("/runs/t3.review"),
            &[],
            "",
            &[],
        );
        assert!(
            text.contains(
                "The gate runs no checks of its own here: no verifier is detected in \
                 this tree, so a compile error or a failing test is yours to name."
            ),
            "{text}"
        );
        assert!(
            !text.contains("While you read, the gate runs the project's own checks"),
            "nothing checks the tree, so the prompt must not claim otherwise: {text}"
        );
    }

    /// The reader sees the same pages the worker did, verbatim, between the
    /// plan of record and the task block; an absent one says so rather than
    /// refusing the reading (rulings 1 and 2 of m1-wiki-first).
    #[test]
    fn the_reader_sees_the_pages_the_plan_named() {
        let pages = vec![
            ("run".to_string(), Some("The run drives waves.".to_string())),
            ("gone".to_string(), None),
        ];
        let text = prompt(
            "# plan: gate-reviewer\n",
            &task(),
            "diff --git a/x b/x\n+fixed\n",
            " x | 1 +\n",
            Path::new("/state/wt/_integration"),
            Path::new("/runs/t3.review"),
            &pages,
            "rust: cargo test",
            &[],
        );
        let plan = text.find("# plan: gate-reviewer").unwrap();
        let heading = text.find("## Pages the plan names").unwrap();
        let run = text.find("The run drives waves.").unwrap();
        let gone = text.find("This project has no such page.").unwrap();
        let block = text.find("## The task\n").unwrap();
        assert!(
            plan < heading && heading < run && run < gone && gone < block,
            "{text}"
        );
    }

    /// A task sent back after a fix carries its earlier readings, so a
    /// second pass verdicts each earlier finding before it reads for
    /// anything else (ruling 3).
    #[test]
    fn a_task_reviewed_before_carries_its_earlier_readings() {
        let earlier = vec![
            "**VERDICT: fix**\n1. app/t1.php:1 -- says draft; the Done line wants the fix.\n"
                .to_string(),
        ];
        let text = prompt(
            "# plan: gate-reviewer\n",
            &task(),
            "diff --git a/x b/x\n+fixed\n",
            " x | 1 +\n",
            Path::new("/state/wt/_integration"),
            Path::new("/runs/t3.review"),
            &[],
            "rust: cargo test",
            &earlier,
        );
        let heading = text.find("## Earlier readings of this task").unwrap();
        let reading = text.find("### Reading 1").unwrap();
        let finding = text.find("says draft").unwrap();
        let instruction = text
            .find("First verdict each earlier finding: addressed or not, with file and line.")
            .unwrap();
        let block = text.find("## The task\n").unwrap();
        assert!(
            heading < reading && reading < finding && finding < instruction && instruction < block,
            "{text}"
        );
        assert!(
            text.contains(
                "Then read the whole diff again for anything else, and name every \
                 instance of a class you find, not the first: each reading costs a round."
            ),
            "{text}"
        );
    }

    /// No earlier reading, no section: a task's first reading is not told to
    /// verdict findings that do not exist.
    #[test]
    fn a_first_reading_carries_no_earlier_readings_section() {
        let text = prompt(
            "# plan: gate-reviewer\n",
            &task(),
            "diff --git a/x b/x\n+fixed\n",
            " x | 1 +\n",
            Path::new("/state/wt/_integration"),
            Path::new("/runs/t3.review"),
            &[],
            "rust: cargo test",
            &[],
        );
        assert!(!text.contains("## Earlier readings of this task"), "{text}");
    }

    #[test]
    fn a_diff_past_the_cap_goes_in_as_its_stat() {
        let big = "+".repeat(DIFF_CAP + 1);
        let text = prompt(
            "# plan: p\n",
            &task(),
            &big,
            " x | 1 +\n",
            Path::new("/wt"),
            Path::new("/r"),
            &[],
            "rust: cargo test",
            &[],
        );
        assert!(text.contains(" x | 1 +"), "the stat stands in");
        assert!(
            text.contains("read the files themselves"),
            "and the reviewer is told to read"
        );
        assert!(!text.contains(&big[..1000]), "the diff body is out");
    }

    #[test]
    fn the_deadline_is_fractional_minutes_with_a_floor_of_one_second() {
        assert_eq!(deadline_from(None), 900);
        assert_eq!(
            deadline_from(Some("0.05")),
            3,
            "three seconds, the way a test injects one"
        );
        assert_eq!(deadline_from(Some("0.001")), 1, "never zero");
        assert_eq!(deadline_from(Some("nonsense")), 900);
        assert_eq!(deadline_from(Some("-2")), 900);
    }

    #[test]
    fn a_provider_limit_is_read_off_the_line_that_says_so() {
        assert_eq!(
            provider_limit("You've reached your Fable limit for this session."),
            Some("You've reached your Fable limit for this session.".into())
        );
        assert_eq!(
            provider_limit("Reading the diff...\n\nError: rate limit exceeded, retry later\n"),
            Some("Error: rate limit exceeded, retry later".into())
        );
        assert_eq!(
            provider_limit("VERDICT: fix\n1. run.rs:10 -- off by one"),
            None,
            "a clean answer names no provider"
        );
    }
}
