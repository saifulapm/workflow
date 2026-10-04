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

/// An open finding as the loop works it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// The short id, which names the fix plan and the owner's question.
    pub id: String,
    pub milestone: String,
    pub step: String,
}

/// The findings of `mem finding list --open --json`, oldest first: the
/// listing dates items by day only, so the order is the ULID's.
fn open_findings(listing: &str) -> Vec<Finding> {
    let items = serde_json::from_str::<serde_json::Value>(listing)
        .ok()
        .and_then(|v| v.get("items").and_then(|i| i.as_array()).cloned())
        .unwrap_or_default();
    let field = |i: &serde_json::Value, k: &str| {
        i.get(k)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string()
    };
    let mut found: Vec<(String, Finding)> = items
        .iter()
        .filter(|i| !field(i, "short_id").is_empty())
        .map(|i| {
            let f = Finding {
                id: field(i, "short_id"),
                milestone: field(i, "milestone"),
                step: field(i, "step"),
            };
            (field(i, "id"), f)
        })
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found.into_iter().map(|(_, f)| f).collect()
}

/// The open finding with this short id.
pub fn finding(listing: &str, id: &str) -> Option<Finding> {
    open_findings(listing)
        .into_iter()
        .find(|f| f.id.eq_ignore_ascii_case(id))
}

/// The finding the loop takes next: the oldest open one that is not in
/// `held`, one id a line, and that no pending question of `mem questions
/// --for human --json` names, since the owner is settling that one.
pub fn oldest(listing: &str, held: &str, questions: &str) -> Option<Finding> {
    let pending: Vec<String> = question_rows(questions)
        .iter()
        .filter(|q| q.get("answer").is_none_or(|a| a.is_null()))
        .filter_map(|q| q.get("body").and_then(|b| b.as_str()))
        .map(str::to_lowercase)
        .collect();
    open_findings(listing).into_iter().find(|f| {
        let named = format!("#{}", f.id.to_lowercase());
        !held.lines().any(|l| l.trim().eq_ignore_ascii_case(&f.id))
            && !pending.iter().any(|body| body.contains(&named))
    })
}

fn question_rows(questions: &str) -> Vec<serde_json::Value> {
    serde_json::from_str::<serde_json::Value>(questions)
        .ok()
        .and_then(|v| v.get("questions").and_then(|q| q.as_array()).cloned())
        .unwrap_or_default()
}

/// The finding's row of `mem finding list --open`, with the instruction
/// that sends its fix to a plan of its own: a ticked roadmap has no plan of
/// record for a task to be added to.
pub fn lead_rows(rows: &str, id: &str) -> String {
    let row: String = rows
        .lines()
        .filter(|l| {
            l.split_whitespace()
                .next()
                .is_some_and(|w| w.eq_ignore_ascii_case(&format!("#{id}")))
        })
        .map(|l| format!("{l}\n"))
        .collect();
    let slug = fix_slug(id);
    format!(
        "{row}\nThe roadmap is in maintenance: store the fix as one plan, `mem plan {slug} --stdin`, and never `mem plan --add-task`.\n"
    )
}

/// The owner's question for a plan over the limit.
pub fn run_question(id: &str, tasks: usize, why: &str) -> String {
    format!(
        "Run fix plan {} for finding #{id}? {tasks} tasks, {why}",
        fix_slug(id)
    )
}

/// The short id `mem ask` prints, `#<id>`.
pub fn asked_id(out: &str) -> Option<String> {
    let word = out.split_whitespace().next()?.strip_prefix('#')?;
    (!word.is_empty()).then(|| word.to_string())
}

/// The answer to the question with this short id, off `mem questions --for
/// human --json`; none while it is pending.
pub fn answer(questions: &str, id: &str) -> Option<String> {
    question_rows(questions)
        .iter()
        .find(|q| {
            q.get("short_id")
                .and_then(|s| s.as_str())
                .is_some_and(|s| s.eq_ignore_ascii_case(id))
        })
        .and_then(|q| q.get("answer").and_then(|a| a.as_str()))
        .map(|a| a.trim().to_string())
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

    const OPEN: &str = r#"{"items":[
        {"id":"01M3","short_id":"CC000003","milestone":"m1","step":"2"},
        {"id":"01M1","short_id":"AA000001","milestone":"m1","step":"1"},
        {"id":"01M2","short_id":"BB000002","milestone":"m2","step":"0"}
    ]}"#;

    #[test]
    fn the_loop_takes_the_oldest_finding_not_held_nor_asked_of_the_owner() {
        let none = r#"{"questions":[]}"#;
        let oldest_id = |held: &str, q: &str| oldest(OPEN, held, q).map(|f| f.id);
        assert_eq!(oldest_id("", none).as_deref(), Some("AA000001"));
        assert_eq!(oldest_id("AA000001\n", none).as_deref(), Some("BB000002"));
        let asked = r#"{"questions":[
            {"body":"finding #aa000001: is it the build year?","answer":null},
            {"body":"Run fix plan fix-bb000002 for finding #BB000002? 4 tasks, more than three","answer":"hold"}
        ]}"#;
        assert_eq!(oldest_id("", asked).as_deref(), Some("BB000002"));
        assert_eq!(oldest_id("AA000001\nBB000002\nCC000003\n", none), None);
        assert_eq!(
            finding(OPEN, "bb000002"),
            Some(Finding {
                id: "BB000002".into(),
                milestone: "m2".into(),
                step: "0".into()
            })
        );
    }

    #[test]
    fn the_lead_gets_its_finding_and_the_plan_to_store() {
        let rows = "#AA000001  open  m1  1  the logo is gone\n#BB000002  open  m2  0  no launch\n";
        let text = lead_rows(rows, "AA000001");
        assert!(
            text.starts_with("#AA000001  open  m1  1  the logo is gone\n"),
            "{text}"
        );
        assert!(!text.contains("BB000002"), "{text}");
        assert!(text.contains("`mem plan fix-aa000001 --stdin`"), "{text}");
    }

    #[test]
    fn the_owners_question_is_read_by_its_short_id() {
        assert_eq!(
            run_question("AB12CD34", 4, "more than three"),
            "Run fix plan fix-ab12cd34 for finding #AB12CD34? 4 tasks, more than three"
        );
        assert_eq!(asked_id("#QX7Z\n").as_deref(), Some("QX7Z"));
        assert_eq!(asked_id(""), None);
        let q = |a: &str| format!(r#"{{"questions":[{{"short_id":"QX7Z2M4P","answer":{a}}}]}}"#);
        assert_eq!(answer(&q("null"), "QX7Z2M4P"), None);
        assert_eq!(answer(&q(r#""run""#), "qx7z2m4p").as_deref(), Some("run"));
        assert_eq!(answer(&q(r#""run""#), "OTHER000"), None);
    }
}
