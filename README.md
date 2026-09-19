# workflow

Three binaries that run a solo development workflow, plus the thin surfaces
that carry it into an editor session.

- `mem/` is the system of record for project state: durable facts, rulings,
  logs, handoffs, blocking questions and a wiki of design pages, kept as
  markdown outside every project repo and synced between machines.
- `workflow/` is the gate and the orchestrator: `verify`, `lint-msg`,
  `review-needed`, plan-driven `run`, `status`, `reap`, `skill`, `doctor`,
  and the body of the git hook stubs.
- `hub/` is a small web view over mem, served tailnet-only, so a phone can
  answer an open question and read any project's memory: `/p/<project>`
  shows its status, handoff, roadmap, plan, stored plans, log, rulings,
  questions with their answers, wiki pages and the runs on that machine.
- `skills/` holds the session-facing instructions (route, plan, roadmap,
  implement, review, orchestrate, mem, unslop). `hooks/` holds the three git
  hook stubs. Another runtime joins by reading the same skills and marking its
  sessions for the gate: `WORKFLOW_AGENT=1`, or pi's own `PI_CODING_AGENT`,
  which the gate reads too because pi has no way to set an environment
  variable per session.

Install each binary twice, because the machines run them from two places:
`cargo install --force --path <crate> --root ~/.local` for the hooks and
hub.service, `cargo install --path <crate>` for the shell. The skills and
the git hook stubs ride inside the workflow binary: `workflow doctor --fix`
writes them to `~/.claude/skills`, `~/.agents/skills` (what pi, codex and
opencode read) and `~/.config/git/hooks`, and reports a copy that has
drifted from the binary until it is run again. The dotfiles call it after
every build, so it is the step after `cargo install` on any machine. Tests
are `cargo test` per crate plus `bash tests/run.sh`. `workflow doctor` and
`mem doctor` check a machine's wiring.

## Starting a project (new or existing)

The gate and the hooks are global, put in place once per machine by
`workflow doctor --fix` (the dotfiles run it after the build); a project only
needs three things, none of them committed:

    cd ~/Sites/github/thing
    mem log "picking this up"        # first write registers the project

    # the route lines, kept out of git for good:
    printf 'AGENTS.md\nCLAUDE.md\n.claude/\n' >> .git/info/exclude
    $EDITOR AGENTS.md                # copy the block from any other project
    printf '@AGENTS.md\n' > CLAUDE.md  # pi, codex and opencode read AGENTS.md; Claude Code imports it

    mem project set verify "pnpm test"          # what green means here
    mem project set review-paths "scripts/**"   # extra risky globs, optional
    mem project set review-model fable          # a reader at the merge gate, if it beats the workers'
    mem project set review-model none           # nobody reads; the run starts unread

From then on every session in that directory gets the project's memory at
start, the commit gate is armed, and the routing skill decides lanes.

A project's spec lives in the wiki, never the repo: write it as pages with
`mem wiki <slug> --stdin`, one per subsystem. A plan names a page with
`Read: wiki:<slug>` instead of restating it, and the run inlines that page,
verbatim, in the worker's brief and the reader's prompt.

A monorepo holds one project per app beside the root project:

    mem project add apps/thing               # a child project at that subdir
    cd apps/thing && mem project set verify "pnpm test --filter=thing"

Sessions inside `apps/thing` resolve to the child — its own plan, status,
handoff, wiki and questions — and everything else in the checkout stays with
the root. The pre-commit gate runs at the toplevel, so it always answers to
the root project's verify.

## Which projects see the skills

A project mem knows. That is the whole answer, and it is the same answer to
"which projects does mem speak in".

The skills are not files on disk. `workflow skill` lists the seven this binary
carries, one `name — description` line each; `workflow skill route` prints that
one whole; `mem skill mem` does the same for the skill about mem. `mem context`
names them in the digest it injects at the start of a session, so a session
learns which skills exist, and what each is for, exactly where mem has a
project to talk about.

Outside such a project — a checkout mem has never been told about, or a
directory that is not a checkout at all — mem says nothing and the skills go
unnamed. Registering a project is how you opt in:

    mem log "first note"        # a write registers this checkout

There used to be a switch per harness for this: `skillOverrides` in Claude
Code's settings, a `skills` array in pi's, two grammars, a trust rule, and a
doctor check to catch a copy on disk that had drifted from the binary. All of
it is gone. `workflow doctor --fix` takes the old copies back off disk, leaving
anything you edited by hand where it is.

## Daily use

Three sizes of work, three moves:

- **Small change** — just ask a session for it. Route makes it a one-shot:
  implement, `workflow verify`, one commit, one `mem log` line. No ceremony.
- **Feature** — say "plan this". Answer one round of questions, approve the
  plan once, and let the session build it task by task. Measured
  2026-09-15: one strong session landed three milestones in the time a run
  spent on one task's fix rounds, so a run is for the case below only.
- **A whole plan in parallel** — for a plan whose wide wave holds three or
  more tasks that share no files, on a machine that carries that many
  workers (up to five), with workers on the
  strongest model and no reader unless the project's numbers earn one: tell
  a session "orchestrate the <plan-id> run". It starts `workflow run`, reads `workflow status`, decides retries
  and cleanup itself, and asks you only what is genuinely yours. Workers are
  amx agents in tmux panes, listed by `amx ls` and watched with
  `amx attach <id>`. Workers run
  on opus unless the project says otherwise (`mem project set model sonnet`)
  or one run does (`WORKFLOW_MODEL=sonnet workflow run`). A cheaper model
  can be given more reasoning: `mem project set effort max` starts every
  worker with `--effort max`, `review-effort` does the same for the reader,
  `unset` takes either back, and `WORKFLOW_EFFORT` and
  `WORKFLOW_REVIEW_EFFORT` do it for one run (empty means no flag). With a
  `review-model` set, every task's diff is read by that model against the
  plan before it merges, after its Verify is green. The reader tags each
  finding `[blocks]` or `[later]`; a fix verdict sends the task back into
  its worker's own session with the findings, a second one to a fresh
  session on the `fix-model` (default: the reader's), and a third is the
  orchestrator's: `workflow accept <task>` lands it with the findings filed
  as follow-ups. `[later]` findings on a ship are follow-ups too. The run
  does not wait on the reading: the task sits `reviewing` while the next
  one is dispatched, and its merge is recorded when the verdict says ship.
  The session that owns the run waits with `workflow wait`, which returns
  the moment a question, a failure or the end needs it. Name the model the workers already run on,
  under any spelling (`opus` and `claude-opus-5` are one model), and nothing
  is read — a model goes over its own work with its own blind spots — so the
  reading costs a session only where it can find something.

A diff can be read before it is committed. `workflow read` starts the
gate's own reader, cold, over the working tree (`--range <r>` for a range)
and prints its verdict: exit 0 ships, 1 is fix, 2 is nobody named to read,
3 is a reading that ended with no verdict. `--against "<one sentence>"` or
`--against wiki:<slug>` says what the diff is held to; without it the plan
of record stands in. Route sends a one-shot past sixty changed lines through
it before the commit, and the review skill is this verb.

A worker can ask a stronger model without stopping. From a task worktree,
`workflow advise "<question>" --file <path>` sends the plan's prose, the
task block, the pages it names, the worker's own reports and the question to
the run's advisor (`WORKFLOW_ADVISOR` for one run, else the reader's model),
prints the answer, and the task goes on; the brief says when to ask. Three
consults an attempt, and a decision (scope, taste, a broken plan) is still
`mem ask`. Outside a run, `--against "<text>"` says what the question is
held to.

Questions find you: on screen while a machine is watched, on the phone
(ntfy via hub) when everything is locked. Answer in the session, with
`mem answer <id> "..."`, or from the hub page. A worker's question never
gets that far. Asked from a task worktree it is addressed to the
orchestrator, which answers it from the plan and the code, and the run
dispatches the task again with the answer in its brief. What reaches you is
only what the orchestrator could not settle, asked fresh in its own words.

## A project planned whole

Bigger than one plan: cut a roadmap of milestones once, with a full plan for
each, and pick them up one at a time.

    mem roadmap --set-file roadmap.md       # the milestones, in the plan grammar
    mem plan m1-auth --set-file m1-auth.md  # one milestone's plan, filed by slug
    mem plan --list                         # what is stored and waiting

    mem roadmap                             # which milestone is next
    mem plan --from m1-auth                 # make it the plan of record

A stored plan's first line has to be `# plan: <slug>`, so a plan is always
filed under the name the roadmap calls it. `--from` is refused while the plan
of record still holds an unchecked task, because that plan is a run in
flight; `mem plan --clear` is the way to abandon it. Ticks are progress on
both files — `mem plan --tick <task>` and `mem roadmap --tick <slug>` — and
`mem context` opens every session with the roadmap heading and the milestone
that is next.

## Reading a project's state

    mem wiki                     # the project's pages, one line each
    mem roadmap                  # the milestones, [x] ticks are progress
    mem plan                     # the active plan, [x] ticks are progress
    mem plan --list              # the milestone plans waiting their turn
    mem log                      # what happened, newest first
    mem log --kind ruling        # decisions taken instead of asking you
    mem search friction --type friction   # exactly what is queued next
    mem show <id>                # the full item behind any #id
    mem questions                # what is waiting on you; --for orchestrator, what a run's workers wait on
    mem handoff                  # where the last session stopped
    mem status                   # the standing one-paragraph status
    workflow status              # a live run: per-task states and reports

The concrete case: to see what amx v3 should fix, run
`mem search friction --type friction --project amx` — every finding is there
with what happened and what was expected. `mem context` opens every session
with the same digest, so a fresh session already knows.

All of it works from anywhere with `--project <name>`, and from any machine:
the store syncs. `mem projects` is the portfolio view.

## The wiki

An item records what happened; a page records how something works now. Every
project can keep both. Pages are markdown in the store, one per subsystem,
linked to each other as `[name](name.md)` and listed in a page called `index`.

    mem wiki cart-pricing          # read this before touching cart pricing
    mem wiki cart-pricing --stdin --note "rounding moved into the service" <page.md

A session reads the page for what it is about to change, then rewrites it when
the change lands. The note is mandatory and becomes a log line, and that log
is the page's history: there are no revisions to dig through. Nothing is
deleted, because a deletion comes back on the next sync, so a page that is
done becomes a one-line stub pointing at what replaced it. `mem doctor`
reports dead links, pages missing from the index and pages big enough to want
compacting.

## Pausing, moving, finishing

- **Leaving a machine mid-work**: commit or stash, and
  `mem handoff --set "..."` says what is next; the handoff names the machine
  the work is on. An orchestrated task that fails keeps its branch in the
  project repo, and the run says which one.
- **Finishing**: merge, push when you decide to (the pre-push stub makes a
  push deliberate: `WORKFLOW_ALLOW_PUSH=1 git push`), then
  `mem status --set "shipped; next decision is ..."` so the project's
  memory says so.
- A finished project costs nothing: its memory stays queryable for ever and
  its repo carries no trace of any of this.

## Migrating a project from another workflow

1. Delete the old process files from the repo (`.focus/`, journals, plan
   files) — git history is history; the point is the working tree.
2. Move the knowledge worth keeping into mem: each decision or gotcha as one
   `mem save`, the current state as `mem status --set`, the next action as
   `mem handoff --set`. Skip anything the code or git log already says.
3. Do the three-step start above (register, AGENTS.md via info/exclude,
   verifier).
4. Commit the deletions in ordinary voice; the gate is already watching.

## How the workflow improves itself

When anything here gets in the way — a gate misfires, a question is
unreadable, a tool is missing — any agent (or you) files one line:

    mem save --project workflow --type friction "friction: what - where - expected"

Product findings go against their own project the same way (that is where
the amx v3 queue came from). Nothing is fixed mid-task. When a few pile up,
say "batch review": one session verifies each claim against the code,
fixes what is real, declines the rest with a recorded ruling, and empties
the queue. Rulings you disagree with are cheap to overturn — that is what
they are for.
