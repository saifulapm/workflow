---
name: orchestrate
description: Use when `workflow go` starts you on a milestone, or when asked to build an approved milestone end to end, from its plan to landed, reviewed and walked code.
---

# orchestrate

You own one milestone, from its approved plan to landed code, and then
you hand the next milestone to a fresh orchestrator. Decide what is yours
to decide, and keep going. The tests are the check.

Speed comes from three habits. Run only the tests a change touches, run the
whole suite once per wave, and decide instead of waiting. A suite on the
critical path of one task stalls the whole milestone, so never put it there.

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

Then read the tree. `git status` must be clean, and the checkout on
`main`, which is then the trunk. The one exception is the worktree recorded
for this project: when `mem project current --json` shows a `worktree`
equal to its `root`, the branch it is on (`git branch --show-current`) is
the trunk. On any other branch, or a detached HEAD, build nothing: ask
with `mem ask` and park (§8). Never switch the checkout's branch, or check
out `main`, to get past this or a `workflow go` refusal; the branch may be
another project's trunk. Read the trunk once and use it wherever
this skill says trunk. `git log --oneline -5` tells you where you are. Find the test command
with `mem project current --json` (the `verify` key) or the repo's own
scripts, and run it once.
A red trunk is yours to fix first, with a failing test that names the cause.
Read code the way the tdd skill does: an outline or a search first, then
only the region. For a wide read, such as every use of a name or what a
module does, send a `scout` subagent and read the lines it cites.

When the handoff or a ticked checklist says this milestone is half done,
continue from the first open task. Never redo landed work. A `worktree-*`
branch with commits and no tick is a dispatched task only when this
project's `mem log` names it in a `dispatched ... on worktree-<name>` line:
read it and land it as §2 says. Leave every other `worktree-*` branch
alone. The branches belong to the whole repo, and another orchestrator
working on another app in another worktree owns it.

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
schema, migration, generated file, port or contract: a type, record, route,
config key or binary that one task changes and the other reads. Run such a
pair in order, or pin the shared names in both briefs. Groundwork a slice
needs (a rename, a moved module, a new seam) is its own first task, and a
change that breaks many callers lands as expand, migrate in batches, then
contract. When the plan is wrong (a
behaviour is already there, a decision contradicts the code), correct it
and record the correction: `mem decide "<what and why>" --by agent`.

Each lesson on this project's `lessons` page whose first line is `Check:`
with no enforcer named becomes your first task: a test, lint or hook that
goes red on the old mistake. Once it lands, shrink the lesson to `Check:
enforced by <path>`.

Done when every behaviour of the plan maps to at least one task, and every
unenforced check lesson has its task.

## 2. Build

Run the first task alone. It proves the recipe: the test command, the
fixtures and the build. Then work the ready set.

**Solo** when the task is small, depends on the last one, or touches shared
files. Use the tdd skill yourself, in the checkout, and commit on the trunk.

**Delegate** when two or more ready tasks are file-disjoint. Spawn
`implementer` subagents with the Agent tool. Run
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

Give pointers, not pasted files. Log each dispatch with its branch, so a
fresh orchestrator can find it: `mem log "dispatched <task> on
worktree-<name> from <sha>"`. The branch is `worktree-agent-<id>`, from the
agent id the launch returns; if the launch does not name it, log the line
when the subagent returns, before landing it. While subagents work, do the
next solo task or write the next brief. A finished subagent is a queue
event, not an interrupt.

**Land each result yourself.** A worktree subagent commits on its own branch,
`worktree-<name>`, cut from the trunk. Its folder is
`.claude/worktrees/<name>` under the repo's main checkout, which is not
your folder when you work in a linked worktree, so find it from the git
common dir. Read its diff (`git diff <trunk>...worktree-<name>`) and check
it stayed in scope and its test asserts the behaviour, not the
implementation. Rerun its targeted test. Then:

    w="$(git rev-parse --path-format=absolute --git-common-dir)/../.claude/worktrees/<name>"
    git -C "$w" rebase <trunk>    # when the trunk moved; retest there
    git merge --ff-only worktree-<name>
    git worktree remove "$w"
    git branch -d worktree-<name>
    mem plan --tick <id>
    mem log "<what landed, in one line>"

Implementers run at medium effort, which suits a precise slice. A subagent
that fails, or returns something you would not merge, gets one fresh
attempt with a better brief at `effort: high`. After that, do the task
yourself.

Done when every task is ticked and landed on the trunk.

## 3. Test cadence

- **Per task.** The test file the task touched, plus the typecheck. Seconds,
  not minutes.
- **Per wave.** After a batch of merges, run the full suite once in the
  background (`run_in_background: true`), with the runner's flag that
  reports every failure (`cargo test --no-fail-fast`), and keep working.
  After merging parallel slices, nothing else lands until that run is
  green: slices that pass alone can fail together. A red suite stops new
  work: find the commit, then follow the diagnose skill. A red that two
  fixes did not turn green parks the milestone (§8).
- **Never in a hook.** The git hooks run only the hygiene check.

A suite slower than about two minutes is a lesson to record, and splitting
or parallelising it is a task worth adding.

## 4. Decide, don't stall

Yours, decided and recorded with `mem decide "<what and why>" --by agent`:
everything reversible inside this repo. That covers names, structure, a
library the plan left open, a test's shape, a correction to the plan, and a
step order. Saiful overturns a recorded decision cheaply. Lost hours cost
more.

Saiful's, asked without waiting, and only his to answer:

    mem ask --for saiful "<question>" --options "<a>,<b>" --recommend "<a>"

These are pushing, deploying or publishing anything; real money or a real
payment; deleting data; a message to a person; a security call (auth,
permissions, secrets); and a taste call the plan and spec leave open, with
no default to take. Ask, then keep working on everything that does not
depend on the answer. Check `mem questions` between tasks.

A message with no tool call ends your turn, and an unattended run stops
there. None of these is a reason to stop: a summary that announces your
next step, an offer to carry on, a list of decisions that blocks nothing,
or a sense that this is a good place to report. Put such text in the same
message as your next tool call, and carry on with whatever does not depend
on Saiful's answer. You stop when the milestone has landed (§7) or parked
(§8).

Never weaken, skip or delete a test to make it pass. Never push.

## 5. Review once

When every task has landed and the suite is green, spawn one `reviewer`
subagent. Give it the commit range of this milestone, the plan slug, the
spec sections and the plan's review focus, word for word. It returns
findings marked `[blocks]` or `[later]`.

Check every finding yourself in the code and drop false or cosmetic ones.
Log each drop with the line that refutes it: `mem log "review finding
dropped: <finding>; <file:line> shows <why>"`.
Fix each real `[blocks]` finding test-first, in one pass with no second
review. A `[later]` finding becomes `mem idea "<finding>"`.

## 6. Dogfood once

Spawn one `dogfooder` subagent. Give it the milestone's Show path (the
roadmap line's `Show:` and the plan's behaviours, as numbered steps
a user takes), the project's launch command, and the `verify` wiki page when
the project has one, and the writes the plan lets one party cause (a form,
a webhook, a message), for its forged-write check. It walks the real product
on an isolated surface and files each defect with `mem finding add`. A UI-free milestone, such as a
library or an internal refactor, has its walk replaced by running its
examples or CLI once yourself.

Fix each finding (`mem finding list --open`) with the diagnose skill. Its
tight loop starts from the finding's evidence, and the fix closes it with
`mem finding close <id> --by <commit>`. A finding no tight loop reproduces
closes with `--by "not reproduced: <what you ran>"`. A taste call goes to
Saiful with `mem ask --for saiful`, an undriveable surface with `mem ask
--for human`. Then spawn the dogfooder once more on
the failed steps and the steps your fixes touched, since a fix can break a
step that passed. Two failed walks of the same step go to Saiful.

## 7. Land and hand over

1. Run the full suite on the trunk and show it green.
2. `mem roadmap --tick <slug>`, then `mem plan --clear`.
3. Keep the lessons. Write down the numbers: wall clock, tasks, solo versus
   delegated, failed attempts, review and dogfood findings, and time spent
   waiting. Then record at most three lessons. A lesson is something a future
   run would otherwise repeat, written as what happened, the rule, and how
   to apply it, under a first line that tags it `Check:` (a test, lint or
   hook could catch the mistake) or `Judgement:`. Lessons about this project
   go on its `lessons` page. Lessons about the workflow go on the workflow
   project's page:

       mem wiki lessons > "$d/l.md"    # add the lesson under its date
       mem wiki lessons --stdin --note "<the lesson in a line>" < "$d/l.md"
       mem --project workflow wiki lessons ...   # the same, for the workflow

   Prefer a check (a test, a lint, a hook) to a skill line, and a skill line
   to a note: the next orchestrator turns each unenforced check into its
   first task. Delete a lesson whose mistake can no longer happen, and keep
   the page under 8 KB.
4. `mem handoff --set "Next: <next milestone>. <slug> landed at <sha>. <the
   numbers>."`
5. `workflow go <project>`. It starts the next milestone's orchestrator in
   its own amx pane, or says the roadmap is done, or refuses because the
   roadmap is not approved yet, or because the folder is another
   project's, or because the project is paused. Show its output. A
   refusal is a normal end, and never a reason to switch a branch.
   Saiful approves the rest from the hub, then presses Start there, which
   has the runner's machine run `workflow go`.

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
could resume the milestone. The hub shows the question. Once he answers
the last question this session asked, the hub has this machine `amx
resume` this very session with his answers as its first message; if amx
no longer has it, a fresh orchestrator starts from your handoff.

## Cloud run

A cloud run is this skill in a claude.ai cloud session
(`CLAUDE_CODE_REMOTE_SESSION_ID` is set), started from its prompt. The
prompt names the milestone, its branch `cloud/<project>-<milestone>` and the
branch to cut it from. Everything above holds, with these changes:

- First check out the cloud branch as the prompt says, never `main`: mem
  here reads the same store the devices read, and it tells this project
  from others in the same repo by that branch. Then
  record the run: `mem project set runner cloud <id> --milestone <slug>`,
  where `<id>` is `$CLAUDE_CODE_REMOTE_SESSION_ID` with `cse_` swapped for
  `session_`.
- The trunk is the cloud branch. Land each task on it as §2 says, then `git push origin
  cloud/<project>-<milestone>` right after its tick, so the branch gains one
  commit, or one fast-forward, per landed task.
- Before every push, run `mem share export` and commit the
  `.mem-outbox.json` it stages, when it stages one: no signal reaches a
  session that ends, so writes the store has not taken yet travel on the
  branch, and the next machine to read the project sends them.
- A task that needs a device, Saiful's desktop, or a login the cloud cannot
  reach is local-only, and its checklist line ends `[local]` (mark it when
  you cut the checklist). Land every task before it that does not wait on
  it, commit nothing for it, push the branch, and write a handoff starting
  `parked: local-only <task>:` that names it and says what it needs. File
  no question: `workflow go` on a device reads that handoff, takes the
  branch and runs the rest. Then stop.
- A question only Saiful can answer parks the run the same way, with
  `mem ask --for saiful` and a handoff starting `parked:` that names it.
  Then stop: the answer comes back as a message in this session.
- Land as §7 says, except that nothing runs `workflow go` and nothing
  pushes `main`, and the branch is pushed before `mem roadmap --tick`: the
  tick tells the mini it may fast-forward, so the review and dogfood fixes
  must be on the branch by then. The run ends with the branch pushed, the
  milestone ticked and the handoff written. The mini fast-forwards its trunk from the branch.
- Stop after 6 hours with a handoff starting `parked: 6-hour stop`. There
  is no amx here, so end by saying so.

## Hygiene

Nothing you write outside the product's code goes in the repo: no plan,
notes, logs, screenshots or checklists. They go to mem, or to a `mktemp -d`
directory while you work. Commit messages and comments follow the tdd skill's voice. The
pre-commit hook runs `workflow hygiene --staged` on every commit. Fix what
it names. Never use a way around it.
