//! Maintenance: an open finding worked through a fix plan of its own, and
//! the limit under which that plan runs without asking the owner.

use crate::ownership;
use crate::plan::Plan;
use crate::review;

/// The most tasks a fix plan may have and still run without asking.
pub const FIX_TASKS: usize = 3;

/// Whether a stored fix plan runs unasked, or the owner is asked first and
/// why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixVerdict {
    Run,
    /// The reason, which follows `<n> tasks,` in the owner's question.
    Ask(String),
}

/// The limit, judged before any diff exists: a plan of more than three tasks
/// asks, and so does one whose `Files:` pattern, or a tracked path that
/// pattern matches, is a review path. `tracked` is the tree's tracked paths,
/// because a pattern like `app/**` is only risky through what it covers.
pub fn fix_verdict(plan: &Plan, tracked: &[String], review_paths: &str) -> FixVerdict {
    let mut why = Vec::new();
    if plan.tasks.len() > FIX_TASKS {
        why.push("more than three".to_string());
    }
    if let Some(path) = risky_path(plan, tracked, review_paths) {
        why.push(format!("{path} is a review path"));
    }
    if why.is_empty() {
        FixVerdict::Run
    } else {
        FixVerdict::Ask(why.join("; "))
    }
}

fn risky_path(plan: &Plan, tracked: &[String], review_paths: &str) -> Option<String> {
    for task in &plan.tasks {
        for pattern in ownership::split_patterns(task.files.as_deref().unwrap_or("")) {
            if let Some(path) = review::risky(std::slice::from_ref(&pattern), review_paths) {
                return Some(path);
            }
            // Ownership matches case and all, so the paths a pattern claims
            // are found the same way here.
            let claimed: Vec<String> = tracked
                .iter()
                .filter(|p| review::glob_match(&pattern, p))
                .cloned()
                .collect();
            if let Some(path) = review::risky(&claimed, review_paths) {
                return Some(path);
            }
        }
    }
    None
}

/// Where a finding stands in the loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixPhase {
    /// A lead is storing the fix plan.
    Lead,
    /// The owner was asked to run a plan over the limit; the question's id.
    Asked(String),
    /// The fix plan is the one serve runs, and its run is going.
    Run,
    /// The run landed and the finding's step is walked again.
    Walk,
}

/// The finding serve is working and its phase, kept as one line so a
/// restarted serve picks it up where it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixState {
    pub finding: String,
    pub phase: FixPhase,
}

impl FixState {
    /// `<finding> <phase>`, the phase `lead`, `asked <question id>`, `run` or
    /// `walk`; anything else is not a state.
    pub fn read(line: &str) -> Option<FixState> {
        let (finding, phase) = line.trim().split_once(' ')?;
        let phase = match phase {
            "lead" => FixPhase::Lead,
            "run" => FixPhase::Run,
            "walk" => FixPhase::Walk,
            other => FixPhase::Asked(other.strip_prefix("asked ")?.to_string()),
        };
        Some(FixState {
            finding: finding.to_string(),
            phase,
        })
    }

    pub fn line(&self) -> String {
        let phase = match &self.phase {
            FixPhase::Lead => "lead".to_string(),
            FixPhase::Asked(question) => format!("asked {question}"),
            FixPhase::Run => "run".to_string(),
            FixPhase::Walk => "walk".to_string(),
        };
        format!("{} {phase}", self.finding)
    }
}

/// The name a finding's fix plan is stored under.
pub fn fix_slug(finding: &str) -> String {
    format!("fix-{}", finding.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan;

    fn plan_of(tasks: &[&str]) -> Plan {
        let mut text = String::from("# plan: fix-ab12cd34\n\n");
        for (i, files) in tasks.iter().enumerate() {
            text.push_str(&format!(
                "- [ ] t{i} Fix part {i}\n      Files: {files}\n      Verify: true\n"
            ));
        }
        plan::parse(&text, true).expect("the plan parses")
    }

    fn tracked(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|p| p.to_string()).collect()
    }

    #[test]
    fn three_tasks_on_plain_paths_run_and_a_fourth_asks() {
        let three = plan_of(&[
            "app/Cart.php",
            "app/Totals.php",
            "resources/views/cart.blade.php",
        ]);
        assert_eq!(fix_verdict(&three, &[], ""), FixVerdict::Run);

        let four = plan_of(&[
            "app/Cart.php",
            "app/Totals.php",
            "app/Tax.php",
            "app/Fee.php",
        ]);
        match fix_verdict(&four, &[], "") {
            FixVerdict::Ask(why) => assert!(why.contains("more than three"), "{why}"),
            v => panic!("four tasks ran: {v:?}"),
        }
    }

    #[test]
    fn a_billing_pattern_asks_and_names_the_path() {
        let plan = plan_of(&["app/Cart.php", "app/billing/tax.php"]);
        match fix_verdict(&plan, &[], "") {
            FixVerdict::Ask(why) => assert!(why.contains("app/billing/tax.php"), "{why}"),
            v => panic!("a billing path ran: {v:?}"),
        }
    }

    #[test]
    fn a_tracked_path_under_the_projects_review_paths_asks() {
        let plan = plan_of(&["packages/**"]);
        let tree = tracked(&["packages/shopify-core/src/Client.ts", "app/Cart.php"]);
        assert_eq!(fix_verdict(&plan, &tree, ""), FixVerdict::Run);
        match fix_verdict(&plan, &tree, "\"packages/shopify-core/**\"") {
            FixVerdict::Ask(why) => {
                assert!(why.contains("packages/shopify-core/src/Client.ts"), "{why}")
            }
            v => panic!("a review path ran: {v:?}"),
        }
    }

    #[test]
    fn every_phase_reads_back_as_written() {
        for phase in [
            FixPhase::Lead,
            FixPhase::Asked("QX7Z2M4P".into()),
            FixPhase::Run,
            FixPhase::Walk,
        ] {
            let state = FixState {
                finding: "AB12CD34".into(),
                phase,
            };
            let line = state.line();
            assert_eq!(FixState::read(&line), Some(state), "{line}");
        }
        assert_eq!(
            FixState::read("AB12CD34 asked QX7Z2M4P\n").map(|s| s.phase),
            Some(FixPhase::Asked("QX7Z2M4P".into()))
        );
        assert_eq!(FixState::read("AB12CD34"), None);
        assert_eq!(FixState::read("AB12CD34 dancing"), None);
    }

    #[test]
    fn the_slug_is_fix_and_the_short_id_in_lowercase() {
        assert_eq!(fix_slug("AB12CD34"), "fix-ab12cd34");
    }
}
