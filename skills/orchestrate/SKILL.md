---
name: orchestrate
description: Use when `workflow go` starts you on a milestone, or when asked to build an approved milestone end to end. Plan the order, build test-first alone or with worktree subagents, land on main, review once, dogfood once, keep the lessons, then start the next milestone's orchestrator.
---

# orchestrate

You own one milestone, from its approved plan to landed code, and then
you hand the next milestone to a fresh orchestrator. Think before each move,
decide what is yours to decide, and keep going. No gate checks you. The
tests do.

Speed comes from three habits. Run only the tests a change touches, run the
whole suite once per wave, and decide instead of waiting. The old engine
spent 64% of its time waiting on suites, so never put the suite on the
critical path of a single task.

## 0. Start

    mem context
    mem handoff
    mem wiki lessons                      # this project's lessons, if any
    mem --project workflow wiki lessons   # lessons about the workflow itself
    mem plan <slug>                       # the plan for this milestone
    mem plan                              # your checklist, when resuming

The plan is markdown written for you: the goal, the Show path, the
decisions already made, one section per behaviour with its acceptance and
what it waits on, then what is shared and what is out of scope. It names no
files, so find the code yourself. Read the spec sections it cites (`mem wiki
<slug>#<section>`). An `Artifacts` link is a picture Saiful made for this
work; build from the plan's words.

Then read the tree. `git status` must be clean and on `main`, and `git log
--oneline -5` tells you where you are. Find the test command with `mem project
current --json` (the `verify` key) or the repo's own scripts, and run it once.
A red trunk is yours to fix first, with a failing test that names the cause.

When the handoff or a ticked checklist says this milestone is half done,
continue from the first open task. Never redo landed work.

Done when you can say what the milestone delivers, the trunk is green, and
you know the exact command for a targeted test and for the whole suite.

## 1. Cut the checklist

Turn the plan's behaviours into tasks. Each task is a vertical slice that ends
in a passing test, and each is small enough to land in under an hour. Write
them as the current plan, under a first line naming the milestone (mem
refuses a checklist without it, and the hub reads progress from it):

    # plan: <milestone-slug>

    - [ ] t1 Store a scheduled message with its send time
    - [ ] t2 List scheduled messages in the composer  [after: t1]

    mem plan --stdin < "$d/checklist.md"

`$d` is a scratch directory from `mktemp -d`, never the repo. Mark what can run at
once. Two tasks run in parallel only when they share no file, lockfile,
schema, migration, generated file or port. When the plan is wrong (a
behaviour is already there, a decision contradicts the code), correct it
and record the correction: `mem decide "<what and why>" --by agent`.

Done when every behaviour of the plan maps to at least one task.

## 2. Build

Run the first task alone. It proves the recipe: the test command, the
fixtures and the build. Then work the ready set.

**Solo** when the task is small, depends on the last one, or touches shared
files. Use the tdd skill yourself, in the checkout, and commit on `main`.

**Delegate** when two or more ready tasks are file-disjoint. Spawn
`implementer` subagents with the Agent tool, `run_in_background: true`. Run
at most 2 at once for Rust and 3 for JS on this 7.3 GB machine. Pick the
model by difficulty: `opus` for cross-cutting design or subtle logic,
`sonnet` for a precise, well-specified slice. The brief is the product, so
write it whole:

    GOAL        one sentence, the outcome
    SCOPE       paths it may write, paths it must not
    CONTEXT     behaviour numbers, file:line pointers, spec sections, the exact
                signatures it consumes or must provide
    ACCEPTANCE  the failing test to write first and what it asserts
    VERIFY      the exact targeted test command and typecheck
    STOP        what sends it back to you instead of improvising

Give pointers, not pasted files. While subagents work, do the next solo task
or write the next brief. A finished subagent is a queue event, not an
interrupt.

**Land each result yourself.** A worktree subagent commits on its own branch,
`worktree-<name>`, under `.claude/worktrees/<name>`. Read its diff (`git diff
main...worktree-<name>`) and check it stayed in scope and its test asserts
the behaviour, not the implementation. Rerun its targeted test. Then:

    git -C .claude/worktrees/<name> rebase main   # when main moved; retest there
    git merge --ff-only worktree-<name>
    git worktree remove .claude/worktrees/<name>
    git branch -d worktree-<name>
    mem plan --tick <id>
    mem log "<what landed, in one line>"

A subagent that fails, or returns something you would not merge, gets one
fresh attempt with a better brief, on a stronger model. After that, do the
task yourself.

Done when every task is ticked and landed on `main`.

## 3. Test cadence

- **Per task.** The test file the task touched, plus the typecheck. Seconds,
  not minutes.
- **Per wave.** After a batch of merges, run the full suite once in the
  background (`run_in_background: true`) and keep working. A red suite
  stops new work: find the commit, then follow the diagnose skill.
- **Never in a hook.** The git hooks run only the hygiene check.

A suite slower than about two minutes is a lesson to record, and splitting
or parallelising it is a task worth adding.

## 4. Decide, don't stall

Yours, decided and recorded with `mem decide "<what and why>" --by agent`:
everything reversible inside this repo. That covers names, structure, a
library the plan left open, a test's shape, a correction to the plan, and a
step order. Saiful overturns a recorded decision cheaply. Lost hours cost
more.

Saiful's, asked without waiting:

    mem ask --for human "<question>" --options "<a>,<b>" --recommend "<a>"

These are pushing, deploying or publishing anything; real money or a real
payment; deleting data; a message to a person; a security call (auth,
permissions, secrets); and a taste call the plan and spec leave open, with
no default to take. Ask, then keep working on everything that does not
depend on the answer. Check `mem questions` between tasks.

Never weaken, skip or delete a test to make it pass. Never push.

## 5. Review once

When every task has landed and the suite is green, spawn one `reviewer`
subagent. Give it the commit range of this milestone, the plan slug and the
spec sections. It returns findings marked `[blocks]` or `[later]`.

Check every finding yourself in the code and drop false or cosmetic ones.
Fix each real `[blocks]` finding test-first, in one pass with no second
review. A `[later]` finding becomes `mem idea "<finding>"`.

## 6. Dogfood once

Spawn one `dogfooder` subagent. Give it the milestone's Show path (the
roadmap line's `Show:` and the plan's behaviours, as numbered steps
a user takes), the project's launch command, and the `verify` wiki page when
the project has one. It walks the real product on an isolated surface and
files each defect with `mem finding add`. A UI-free milestone, such as a
library or an internal refactor, has its walk replaced by running its
examples or CLI once yourself.

Fix each finding (`mem finding list --open`) with the diagnose skill. Its
tight loop starts from the finding's evidence, and the fix closes it with
`mem finding close <id> --by <commit>`. A finding no tight loop reproduces
closes with `--by "not reproduced: <what you ran>"`. A taste call or an undriveable surface goes to
Saiful with `mem ask --for human`. Then spawn the dogfooder once more on
the failed steps alone. Two failed walks of the same step go to Saiful.

## 7. Land and hand over

1. Run the full suite on `main` and show it green.
2. `mem roadmap --tick <slug>`, then `mem plan --clear`.
3. Keep the lessons. Write down the numbers: wall clock, tasks, solo versus
   delegated, failed attempts, review and dogfood findings, and time spent
   waiting. Then record at most three lessons. A lesson is something a future
   run would otherwise repeat, written as what happened, the rule, and how
   to apply it. Lessons about this project go on its `lessons` page. Lessons
   about the workflow go on the workflow project's page:

       mem wiki lessons > "$d/l.md"    # add the lesson under its date
       mem wiki lessons --stdin --note "<the lesson in a line>" < "$d/l.md"
       mem --project workflow wiki lessons ...   # the same, for the workflow

   A lesson that keeps returning becomes a rule. Prefer a check (a test, a
   lint, a hook) to a skill line, and a skill line to a note.
4. `mem handoff --set "<slug> landed at <sha>. <the numbers>. Next: <next
   milestone>."`
5. `workflow go <project>`. It starts the next milestone's orchestrator in
   its own amx pane, or says the roadmap is done, or refuses because the
   roadmap is not approved yet. Show its output. A refusal is a normal end.
   Saiful approves the rest from the hub, and `workflow go` starts it then.

Done when the goal's conditions all show in this conversation.

## 8. Park

When nothing is left that does not wait on Saiful, park the milestone. File
the open question with `mem ask --for human`, write `mem handoff --set
"parked: <where you stopped, the question id, how to resume>"` and `mem log
"parked: <why>"`, and say so. The handoff must start with `parked:`: that
word is how the hub tells a parked milestone from a stalled one, and a
stalled one rings his phone. Then end your own session with `amx stop
--force "$AMX_ID"`: a session that only stops talking idles at its prompt,
which the hub and `workflow go` read as a live orchestrator, so nothing
could resume the milestone. The hub shows the question. Once he answers,
he presses Resume on the hub of a machine with a checkout, which starts a
fresh orchestrator that reads your handoff and his answer.

## Hygiene

Nothing you write outside the product's code goes in the repo: no plan,
notes, logs, screenshots or checklists. They go to mem, or to a `mktemp -d`
directory while you work. Commit messages and comments follow the tdd skill's voice. The
pre-commit hook runs `workflow hygiene --staged` on every commit. Fix what
it names. Never use a way around it.
