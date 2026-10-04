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

/// The walk going on for a project: `<serve dir>/walk`. Its first line is
/// `<slug> <walk> <started> <session> <step>...`, the steps last because
/// there are as many as the walk covers; a walk that was read and held the
/// milestone has its outcome's [`WalkOutcome::words`] on a second line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkState {
    pub slug: String,
    /// Which walk of the milestone this is, from 1.
    pub walk: usize,
    pub started: i64,
    /// The step numbers this walk covers.
    pub steps: Vec<usize>,
    pub session: String,
    /// What the walk came to, once serve has read it.
    pub outcome: Option<WalkOutcome>,
}

impl WalkState {
    pub fn read(text: &str) -> Option<WalkState> {
        let mut lines = text.lines();
        let mut parts = lines.next()?.split_whitespace();
        Some(WalkState {
            slug: parts.next()?.to_string(),
            walk: parts.next()?.parse().ok()?,
            started: parts.next()?.parse().ok()?,
            session: parts.next()?.to_string(),
            steps: parts.map(|n| n.parse().ok()).collect::<Option<_>>()?,
            outcome: lines
                .next()
                .filter(|l| !l.trim().is_empty())
                .map(outcome_of),
        })
    }

    pub fn line(&self) -> String {
        let steps: String = self.steps.iter().map(|n| format!(" {n}")).collect();
        let outcome = match &self.outcome {
            Some(o) => format!("{}\n", o.words()),
            None => String::new(),
        };
        format!(
            "{} {} {} {}{steps}\n{outcome}",
            self.slug, self.walk, self.started, self.session
        )
    }
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

/// How long a walk's session may run. One alive past it is stopped and the
/// walk skipped, so a hung browser does not hold the project; the bound is a
/// first guess.
pub const WALK_S: i64 = 45 * 60;

/// Why a walk stopped at [`WALK_S`] is skipped.
pub const WALK_TIMED_OUT: &str = "the session ran forty-five minutes";

/// The steps of a milestone's open findings, one per finding, off
/// `mem finding list --open --json`; none when the listing does not parse.
pub fn finding_steps(listing: &str, slug: &str) -> Vec<String> {
    let items = serde_json::from_str::<serde_json::Value>(listing)
        .ok()
        .and_then(|v| v.get("items").and_then(|i| i.as_array()).cloned())
        .unwrap_or_default();
    items
        .iter()
        .filter(|i| i.get("milestone").and_then(|m| m.as_str()) == Some(slug))
        .filter_map(|i| i.get("step").and_then(|s| s.as_str()))
        .map(|s| s.trim().to_string())
        .collect()
}

/// The ids of the milestone's open findings a walk settles, off `mem finding
/// list --open --json`: each on a step it walked and did not report failed.
/// A skipped walk settles none, since it reported nothing of any step.
pub fn fixed_findings(listing: &str, slug: &str, walked: &[usize], o: &WalkOutcome) -> Vec<String> {
    let failed: &[usize] = match o {
        WalkOutcome::Pass => &[],
        WalkOutcome::Failed(steps) => steps,
        WalkOutcome::Skipped(_) => return Vec::new(),
    };
    let settled = |step: &str| {
        step.trim()
            .parse::<usize>()
            .is_ok_and(|n| walked.contains(&n) && !failed.contains(&n))
    };
    let items = serde_json::from_str::<serde_json::Value>(listing)
        .ok()
        .and_then(|v| v.get("items").and_then(|i| i.as_array()).cloned())
        .unwrap_or_default();
    items
        .iter()
        .filter(|i| i.get("milestone").and_then(|m| m.as_str()) == Some(slug))
        .filter(|i| i.get("step").and_then(|s| s.as_str()).is_some_and(settled))
        .filter_map(|i| i.get("id").and_then(|s| s.as_str()))
        .map(str::to_string)
        .collect()
}

/// The milestone's rows of `mem finding list --open`, whose third column is
/// the milestone.
pub fn milestone_rows(listing: &str, slug: &str) -> String {
    listing
        .lines()
        .filter(|l| l.split_whitespace().nth(2) == Some(slug))
        .map(|l| format!("{l}\n"))
        .collect()
}

/// A failed walk held to its findings: a failed step with no open finding
/// skips the walk, since a step the session could not drive is a finding,
/// never a bare claim. `open` is [`finding_steps`] for the milestone.
pub fn held_to_findings(o: WalkOutcome, open: &[String]) -> WalkOutcome {
    let WalkOutcome::Failed(steps) = &o else {
        return o;
    };
    match steps.iter().find(|n| !open.contains(&n.to_string())) {
        Some(n) => WalkOutcome::Skipped(format!("step {n} failed with no finding")),
        None => o,
    }
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
    fn a_walk_reads_back_as_it_was_written() {
        let walk = WalkState {
            slug: "cart".into(),
            walk: 2,
            started: 1791000000,
            steps: vec![2, 3],
            session: "wf-dogfood-a3k9".into(),
            outcome: None,
        };
        assert_eq!(walk.line(), "cart 2 1791000000 wf-dogfood-a3k9 2 3\n");
        assert_eq!(WalkState::read(&walk.line()), Some(walk.clone()));
        let read = WalkState {
            outcome: Some(WalkOutcome::Skipped("no report".into())),
            ..walk
        };
        assert_eq!(
            read.line(),
            "cart 2 1791000000 wf-dogfood-a3k9 2 3\nskipped no report\n"
        );
        assert_eq!(WalkState::read(&read.line()), Some(read));
        assert_eq!(WalkState::read("cart 1 1791000000"), None);
        assert_eq!(WalkState::read("cart 1 1791000000 s x"), None);
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

    #[test]
    fn a_failed_step_counts_only_with_an_open_finding_of_the_milestone() {
        let listing = r#"{"items":[
            {"milestone":"cart","step":"2","file":"evidence/cart/a.png"},
            {"milestone":"home","step":"3","file":null}
        ]}"#;
        let open = finding_steps(listing, "cart");
        assert_eq!(open, ["2"]);
        assert!(finding_steps("not json", "cart").is_empty());
        assert_eq!(
            held_to_findings(WalkOutcome::Failed(vec![2]), &open),
            WalkOutcome::Failed(vec![2])
        );
        assert_eq!(
            held_to_findings(WalkOutcome::Failed(vec![2, 3]), &open),
            WalkOutcome::Skipped("step 3 failed with no finding".into())
        );
        assert_eq!(held_to_findings(WalkOutcome::Pass, &[]), WalkOutcome::Pass);
    }

    #[test]
    fn a_walk_settles_the_findings_on_the_steps_it_walked_clean() {
        let listing = r#"{"items":[
            {"id":"F1","milestone":"cart","step":"2"},
            {"id":"F2","milestone":"cart","step":"3"},
            {"id":"F3","milestone":"cart","step":"1"},
            {"id":"F4","milestone":"home","step":"2"},
            {"id":"F5","milestone":"cart","step":null}
        ]}"#;
        let walked = [2, 3];
        assert_eq!(
            fixed_findings(listing, "cart", &walked, &WalkOutcome::Pass),
            ["F1", "F2"]
        );
        assert_eq!(
            fixed_findings(listing, "cart", &walked, &WalkOutcome::Failed(vec![3])),
            ["F1"]
        );
        assert!(
            fixed_findings(
                listing,
                "cart",
                &walked,
                &WalkOutcome::Skipped("no report".into())
            )
            .is_empty()
        );
    }

    #[test]
    fn a_milestone_keeps_only_its_own_finding_rows() {
        let listing =
            "#F1  open  cart  2  the basket stays empty\n#F2  open  home  1  the logo is gone\n";
        assert_eq!(
            milestone_rows(listing, "cart"),
            "#F1  open  cart  2  the basket stays empty\n"
        );
        assert_eq!(milestone_rows(listing, "shop"), "");
    }
}
