//! The dogfood walk: a landed milestone's Show path cut into steps, the
//! brief's inputs, and what the walk came to, read from the session's
//! report or from a request's answer and written back as the run line.

/// The words for a milestone with no `Show:` line. It rides in
/// [`WalkOutcome::Skipped`] and is written bare, never as `skipped ...`,
/// because the run line and a request's answer say it that way.
pub const NO_SHOW_PATH: &str = "no Show path";

/// A Show line as its steps, in order; step `n` is index `n - 1`. The engine
/// cuts it rather than the session, so a step number means the same in the
/// brief, in `mem finding add --step` and in the walk after a fix.
pub fn show_steps(show: &str) -> Vec<String> {
    show.split(", ")
        .flat_map(|part| part.split("; "))
        .map(|step| {
            let step = step.trim();
            step.strip_prefix("and ")
                .or_else(|| step.strip_prefix("then "))
                .unwrap_or(step)
        })
        .filter(|step| !step.is_empty())
        .map(str::to_string)
        .collect()
}

/// Everything `brief::dogfood` renders.
#[derive(Debug, Clone, Default)]
pub struct WalkBrief {
    pub project: String,
    pub slug: String,
    /// The milestone's `Surface:` value.
    pub surface: String,
    /// The steps to walk with their numbers on the whole Show path: all of
    /// them on a first walk, only the failed ones on a re-walk.
    pub steps: Vec<(usize, String)>,
    /// The Launch, Doctor, Drive, Evidence and Cleanup sections of the
    /// project's `verify` page; empty when the page is missing.
    pub verify: String,
    /// The surface's section of `dogfood-playbooks`; empty when it has none.
    pub playbook: String,
    pub dev: Option<String>,
    pub preview: Option<String>,
    /// The milestone's open findings; empty on a first walk.
    pub findings: String,
}

/// What a walk came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WalkOutcome {
    Pass,
    /// The step numbers the session reported failed.
    Failed(Vec<usize>),
    /// Why the walk counts for nothing, [`NO_SHOW_PATH`] among them.
    Skipped(String),
}

impl WalkOutcome {
    /// The answer to a request: `pass`, `failed <n> <n>`, `no Show path`
    /// or `skipped <why>`.
    pub fn words(&self) -> String {
        match self {
            WalkOutcome::Pass => "pass".into(),
            WalkOutcome::Failed(steps) => steps
                .iter()
                .fold("failed".to_string(), |s, n| format!("{s} {n}")),
            WalkOutcome::Skipped(why) if why == NO_SHOW_PATH => NO_SHOW_PATH.into(),
            WalkOutcome::Skipped(why) => format!("skipped {why}"),
        }
    }
}

/// A status file's last report as the walk's outcome.
pub fn read_outcome(status: &str) -> WalkOutcome {
    let Some((state, note)) = crate::run::last_status_in(status) else {
        return WalkOutcome::Skipped("no report".into());
    };
    if state != "ready" {
        return WalkOutcome::Skipped(format!("the session reported {state}"));
    }
    // Only the two verdicts: a session that could claim skipped or no Show
    // path could end a walk without walking it.
    verdict(note.trim())
        .unwrap_or_else(|| WalkOutcome::Skipped(format!("an unreadable report: {note}")))
}

/// A request's answer, [`WalkOutcome::words`], read back.
pub fn outcome_of(words: &str) -> WalkOutcome {
    let words = words.trim();
    if words == NO_SHOW_PATH {
        return WalkOutcome::Skipped(NO_SHOW_PATH.into());
    }
    if let Some(why) = words.strip_prefix("skipped ") {
        return WalkOutcome::Skipped(why.trim().into());
    }
    verdict(words).unwrap_or_else(|| WalkOutcome::Skipped(format!("an unreadable answer: {words}")))
}

/// `pass`, or `failed` with at least one step number. A bare `failed` is not
/// one: with no step to hold a finding against, it would pass the check that
/// every failed step has one.
fn verdict(text: &str) -> Option<WalkOutcome> {
    if text == "pass" {
        return Some(WalkOutcome::Pass);
    }
    let steps: Vec<usize> = text
        .strip_prefix("failed ")?
        .split_whitespace()
        .map(|n| n.parse().ok())
        .collect::<Option<_>>()?;
    (!steps.is_empty()).then_some(WalkOutcome::Failed(steps))
}

/// The `mem log --type run` line for a walk. A failed walk is counted by
/// `open`, the milestone's open findings, since those are what a lead turns
/// into fix tasks.
pub fn walk_line(slug: &str, o: &WalkOutcome, open: usize) -> String {
    let what = match o {
        WalkOutcome::Failed(_) => format!("findings {open}"),
        o => o.words(),
    };
    format!("dogfood {slug}: {what}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_three_clause_show_line_cuts_into_three_steps() {
        assert_eq!(
            show_steps("open the cart page, add the fixture basket; then the total reads 42.00"),
            [
                "open the cart page",
                "add the fixture basket",
                "the total reads 42.00"
            ]
        );
        assert_eq!(
            show_steps("run `app list`, and the new row shows, "),
            ["run `app list`", "the new row shows"]
        );
        assert_eq!(show_steps("the home page loads"), ["the home page loads"]);
    }

    #[test]
    fn every_answer_word_reads_back_as_its_outcome() {
        for (words, outcome) in [
            ("pass", WalkOutcome::Pass),
            ("failed 3 5", WalkOutcome::Failed(vec![3, 5])),
            ("failed 0", WalkOutcome::Failed(vec![0])),
            ("no Show path", WalkOutcome::Skipped(NO_SHOW_PATH.into())),
            (
                "skipped no report",
                WalkOutcome::Skipped("no report".into()),
            ),
        ] {
            assert_eq!(outcome_of(words), outcome, "{words}");
            assert_eq!(outcome.words(), words);
        }
    }

    #[test]
    fn an_answer_that_is_none_of_the_words_is_skipped() {
        for words in ["", "failed", "failed x", "passed"] {
            assert!(
                matches!(outcome_of(words), WalkOutcome::Skipped(_)),
                "{words:?}"
            );
        }
    }

    #[test]
    fn every_report_line_reads_back_as_its_outcome() {
        let file = |last: &str| {
            format!("--- attempt 1 ---\n2026-10-04T11:30:49Z started walking 3 steps\n{last}\n\n")
        };
        assert_eq!(
            read_outcome(&file("2026-10-04T11:52:01Z ready pass")),
            WalkOutcome::Pass
        );
        assert_eq!(
            read_outcome(&file("2026-10-04T11:52:01Z ready failed 2 3")),
            WalkOutcome::Failed(vec![2, 3])
        );
        assert_eq!(
            read_outcome(&file(
                "2026-10-04T11:52:01Z blocked the dev server never came up"
            )),
            WalkOutcome::Skipped("the session reported blocked".into())
        );
        assert_eq!(
            read_outcome(&file("2026-10-04T11:52:01Z ready all good")),
            WalkOutcome::Skipped("an unreadable report: all good".into())
        );
        // A session may not claim the milestone has no Show path.
        assert_eq!(
            read_outcome(&file("2026-10-04T11:52:01Z ready no Show path")),
            WalkOutcome::Skipped("an unreadable report: no Show path".into())
        );
        for empty in ["", "--- attempt 1 ---\n", "\n\n"] {
            assert_eq!(
                read_outcome(empty),
                WalkOutcome::Skipped("no report".into()),
                "{empty:?}"
            );
        }
    }

    #[test]
    fn each_outcome_has_its_run_line() {
        assert_eq!(
            walk_line("cart", &WalkOutcome::Pass, 0),
            "dogfood cart: pass"
        );
        assert_eq!(
            walk_line("cart", &WalkOutcome::Failed(vec![2]), 3),
            "dogfood cart: findings 3"
        );
        assert_eq!(
            walk_line("cart", &outcome_of("no Show path"), 0),
            "dogfood cart: no Show path"
        );
        assert_eq!(
            walk_line(
                "cart",
                &WalkOutcome::Skipped("step 2 failed with no finding".into()),
                0
            ),
            "dogfood cart: skipped step 2 failed with no finding"
        );
    }
}
