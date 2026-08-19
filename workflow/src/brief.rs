//! The worker brief (spec §8.4): objective, the task block verbatim, the
//! constraints, the mem cheat-line and the reporting protocol, inside 2,000
//! bytes.

use std::path::Path;

use crate::plan::Task;
use crate::warn;

pub fn text(task: &Task, worktree: &Path, status_file: &Path) -> String {
    format!(
        "\
# {id} -- {title}

You are working alone in {wt}. Never leave it.

## The task, as the plan states it

{block}
## How to work

Write the failing test first, then the code that passes it. The Verify: line
above is your evidence command. Commit each atomic change in ordinary
engineering voice -- no trailers, no session links, no words like agent, AI or
orchestration. Stage only the files this task touched; never `git add -A`.
Everything you write must match the Files: patterns; anything outside them is
refused at the merge gate and the task is parked.

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

States: started, progress, ready, blocked. `ready` means merge-ready and is
your last act.
",
        id = task.id,
        title = task.title,
        wt = worktree.display(),
        block = task.block,
        status = status_file.display(),
    )
}

pub fn write(task: &Task, worktree: &Path, status_file: &Path, out: &Path) {
    if let Some(dir) = out.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let body = text(task, worktree, status_file);
    let _ = std::fs::write(out, &body);
    if body.len() > 2000 {
        warn(format!(
            "task {}: the brief is {} bytes, over the 2000 byte budget",
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
        );
        assert!(body.len() <= 2000, "the brief is {} bytes", body.len());
        for needle in [
            "Extract cart pricing into a service",
            "Files: app/Services/Cart*.php tests/Unit/Cart*",
            "Verify: bin/php artisan test --filter=Cart",
            "Done: cart totals identical",
            "Never leave it",
            "never `git add -A`",
            "mem ask",
            "started, progress, ready, blocked",
            "/state/runs/app/plan/t1.status",
        ] {
            assert!(body.contains(needle), "the brief lost {needle}");
        }
    }
}
