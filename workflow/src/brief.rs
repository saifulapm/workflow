//! The worker brief (spec §8.4): objective, the task block verbatim, the
//! constraints, the mem cheat-line and the reporting protocol, inside
//! [`BUDGET`] bytes.

use std::path::Path;

use crate::plan::Task;
use crate::warn;

/// What a brief may weigh. 2,000 bytes held the core keys; the middle-tier
/// keys (Read, Uses, Gives, Pattern) earn the room -- a deliberate deviation
/// from spec §8.4's figure, recorded as a ruling.
pub const BUDGET: usize = 3000;

/// The states a worker may report, in the order the brief teaches them. The
/// gate names this list back when a report uses a word that is not on it, so
/// the two must be the same list (friction #W2SY30WH).
pub const STATES: [&str; 4] = ["started", "progress", "ready", "blocked"];

/// What the attempt before this one came to. A redispatched worker used to
/// wake up to the same fixed text as the first attempt, with the status file
/// truncated behind it, so the only way to tell it anything was to leave a
/// ruling in mem and hope it looked (friction #YCW7ND6Z).
#[derive(Debug, Clone, Default)]
pub struct Prior {
    /// How many attempts have already been made, this one not counted.
    pub attempts: u64,
    /// Why the last one ended, in the run's own words.
    pub why: String,
    /// The last line the last attempt wrote to its status file.
    pub last_report: String,
}

impl Prior {
    /// The section, or nothing at all on a first attempt.
    fn section(&self) -> String {
        if self.attempts == 0 {
            return String::new();
        }
        let mut s = format!(
            "## The attempt before this one\n\nThis is attempt {}.",
            self.attempts + 1
        );
        if !self.why.is_empty() {
            s.push_str(&format!(" The last one ended: {}.", self.why));
        }
        if !self.last_report.is_empty() {
            s.push_str(&format!(" Its last report was '{}'.", self.last_report));
        }
        s.push_str("\nRead what it did before repeating it.\n\n");
        s
    }
}

pub fn text(task: &Task, worktree: &Path, status_file: &Path, prior: &Prior) -> String {
    format!(
        "\
# {id} -- {title}

You are working alone in {wt}. Never leave it. What this
task depends on is already there; never go looking for another branch.

{prior}## The task, as the plan states it

{block}
## How to work

Write the failing test first, then the code that passes it. Your evidence
command is `workflow verify`, which runs the Verify: line above. A red
Verify is answered in the code it tests, never by weakening the test. Commit each
atomic change in ordinary
engineering voice -- no trailers, no session links, no words like agent, AI or
orchestration, no puffery, plain words over fancy ones, straight quotes, no
em dashes. Stage only the files this task touched; never `git add -A`.
Everything you write must match the Files: patterns; anything outside them is
refused at the merge gate and the task is failed.

## Stop and ask -- never decide these yourself

Irreversible change · security-sensitive change · any effect outside this
worktree (push, publish, deploy, external write) · the plan is broken beyond
guessing · credentials or secrets. On any of them: `mem ask \"<question>\"`,
`mem handoff --set \"<where you are>\"`, write a `blocked` line, stop.

## mem

mem log \"<what happened>\"
mem save --kind ruling --type <type> \"<what - why - cost if wrong>\"
mem ask \"<question>\" · mem handoff --set \"<state>\"

## Reporting

Append one line per state change to {status}:

    <utc> <state> <note>

States: {states}. `ready` means merge-ready and is
your last act. The state is one bare word: no colon after it.
",
        id = task.id,
        title = task.title,
        wt = worktree.display(),
        block = task.block,
        prior = prior.section(),
        status = status_file.display(),
        states = STATES.join(", "),
    )
}

pub fn write(task: &Task, worktree: &Path, status_file: &Path, prior: &Prior, out: &Path) {
    if let Some(dir) = out.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let body = text(task, worktree, status_file, prior);
    let _ = std::fs::write(out, &body);
    if body.len() > BUDGET {
        warn(format!(
            "task {}: the brief is {} bytes, over the {BUDGET} byte budget",
            task.id,
            body.len()
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_brief_carries_the_task_and_stays_inside_its_budget() {
        let task = Task {
            id: "t1".into(),
            title: "Extract cart pricing into a service".into(),
            block: "- [ ] t1 Extract cart pricing into a service\n      \
                    Files: app/Services/Cart*.php tests/Unit/Cart*\n      \
                    Verify: bin/php artisan test --filter=Cart\n      \
                    Done: cart totals identical for the fixture basket\n"
                .into(),
            ..Task::default()
        };
        let body = text(
            &task,
            Path::new("/state/worktrees/app/plan/t1"),
            Path::new("/state/runs/app/plan/t1.status"),
            &Prior::default(),
        );
        assert!(body.len() <= 2000, "the brief is {} bytes", body.len());
        for needle in [
            "Extract cart pricing into a service",
            "Files: app/Services/Cart*.php tests/Unit/Cart*",
            "Verify: bin/php artisan test --filter=Cart",
            "Done: cart totals identical",
            "Your evidence\ncommand is `workflow verify`",
            "no puffery",
            "Never leave it",
            "never `git add -A`",
            "mem ask",
            "started, progress, ready, blocked",
            "/state/runs/app/plan/t1.status",
        ] {
            assert!(body.contains(needle), "the brief lost {needle}");
        }
        assert!(
            !body.contains("The attempt before"),
            "a first attempt has no attempt before it: {body}"
        );

        // A middle-tier block -- Read, Uses, Gives, Pattern beside the core
        // keys -- is what the budget has to hold now.
        let rich = Task {
            id: "t2".into(),
            title: "Wire cart pricing into checkout".into(),
            block: "- [ ] t2 Wire cart pricing into checkout [after: t1]\n      \
                    Files: app/Checkout/*.php app/Services/Cart*.php tests/Unit/Checkout*\n      \
                    Read: app/Checkout/Total.php app/Services/CartPricing.php docs/pricing.md\n      \
                    Uses: CartPricing::price(Basket $b): Cents · Basket::fixture(): Basket\n      \
                    Gives: CheckoutTotal::grand(Basket $b): Cents\n      \
                    Pattern: app/Checkout/Shipping.php:40-88\n      \
                    Verify: bin/php artisan test --filter=Checkout\n      \
                    Done: every fixture basket totals identically through checkout and cart\n"
                .into(),
            ..Task::default()
        };
        let rich_body = text(
            &rich,
            Path::new("/state/worktrees/app/plan/t2"),
            Path::new("/state/runs/app/plan/t2.status"),
            &Prior::default(),
        );
        assert!(
            rich_body.len() <= BUDGET,
            "a middle-tier brief is {} bytes, over the {BUDGET} byte budget",
            rich_body.len()
        );
        assert!(
            rich_body.contains("never by weakening the test"),
            "the brief lost the test invariant"
        );

        // The redispatch, which is where the section earns its bytes.
        let prior = Prior {
            attempts: 1,
            why: "wrote outside its Files: patterns".into(),
            last_report: "ready merge-ready".into(),
        };
        let again = text(
            &task,
            Path::new("/state/worktrees/app/plan/t1"),
            Path::new("/state/runs/app/plan/t1.status"),
            &prior,
        );
        assert!(again.len() <= 2000, "the brief is {} bytes", again.len());
        for needle in [
            "This is attempt 2.",
            "wrote outside its Files: patterns",
            "ready merge-ready",
        ] {
            assert!(again.contains(needle), "the redispatch brief lost {needle}");
        }
    }
}
