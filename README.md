# workflow

Three binaries that run a solo development workflow, plus the thin surfaces
that carry it into an editor session.

- `mem/` is the system of record for project state: durable facts, rulings,
  logs, handoffs, blocking questions and a wiki of design pages, kept as
  markdown outside every project repo and synced between machines.
- `workflow/` is the gate and the orchestrator: `verify`, `lint-msg`,
  `hygiene`, `review-needed`, plan-driven `run`, `status`, `reap`, `skill`, `doctor`,
  and the body of the git hook stubs.
- `hub/` is a small web server over mem, served tailnet-only, so a phone can
  follow every project through its lifecycle, answer its questions and press
  its buttons (see The hub below).
- `skills/` holds the session-facing instructions (research, grill, plan,
  work, lead, dogfood, fix, review, garden, route, mem). `hooks/` holds the three git
  hook stubs. Another runtime joins by reading the same skills and marking its
  sessions for the gate: `WORKFLOW_AGENT=1`, or pi's own `PI_CODING_AGENT`,
  which the gate reads too because pi has no way to set an environment
  variable per session.

Install each binary twice, because the machines run them from two places:
`cargo install --force --path <crate> --root ~/.local` for the hooks and
hub.service, `cargo install --path <crate>` for the shell. The skills and
the git hook stubs ride inside the workflow binary: `workflow doctor --fix`
installs them as files in `~/.claude/skills`, `~/.agents/skills` (what pi, codex and
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

    # the route lines, kept out of git by the global ignore list:
    $EDITOR AGENTS.md                # copy the block from any other project
    printf '@AGENTS.md\n' > CLAUDE.md  # pi, codex and opencode read AGENTS.md; Claude Code imports it

    mem project set verify "pnpm test"          # what green means here
    mem project set review-paths "scripts/**"   # extra risky globs, optional

From then on every session in that directory gets the project's memory at
start, the commit gate is armed, and the routing skill decides lanes.

A project's spec lives in the wiki, never the repo: write it as pages with
`mem wiki <slug> --stdin`, one per subsystem. A plan names a page with
`Read: wiki:<slug>` instead of restating it, and the run inlines that page,
verbatim, in the worker's brief.

A monorepo holds one project per app beside the root project:

    mem project add apps/thing               # a child project at that subdir
    cd apps/thing && mem project set verify "pnpm test --filter=thing"

Sessions inside `apps/thing` resolve to the child — its own plan, status,
handoff, wiki and questions — and everything else in the checkout stays with
the root. A run puts the project's name in every worker's environment as
`MEM_PROJECT`, which mem reads when `--project` is absent, so a worker at its
worktree's root still writes to the child. The pre-commit gate runs at the
toplevel, so it always answers to the root project's verify.

## What the gate refuses

A product repo should read as if a person wrote it, so every commit in a
checkout mem knows passes `workflow hygiene`, which looks for agent files and
process references in two tiers. The hard tier refuses: a file on the global
ignore list tracked or staged, a `.gitignore` line naming one, and, in the
added lines of non-markdown files and in commit messages, a numbered ruling,
milestone, ticket, issue or ADR, a mem id, a `Co-Authored-By` or
`Generated with` line. A message also fails on a subject over 72 characters
and on a bare task or milestone id such as `t3`. The soft tier only warns, and
only on messages: lint-msg's style tables and words such as session, agent
and handoff, each cleared for one project by a lint-exception ruling, since
some products use agent as their own vocabulary. The pre-commit hook reads
the staged diff, commit-msg reads the message, and the merge gate reads every
commit of a task branch and names the one to reword. `workflow lint-msg` runs
the message half alone. `workflow hygiene` with no mode reads the whole
tracked tree and the last 200 commits; `--fix` untracks ignore-list files and
drops their `.gitignore` lines.

`WORKFLOW_HYGIENE=skip git commit` lets one of your own commits past the check
with a warning. Nothing clears the hard tier for an agent: the override is
ignored when `WORKFLOW_AGENT` (or pi's `PI_CODING_AGENT`) is set or the commit
is in a run worktree. A project exempts paths from the content checks with
globs, matched like a plan's Files patterns, for tests whose fixtures carry
the very strings the check looks for:

    mem project set hygiene-exempt "tests/**"

The ignore list is global, through git's `core.excludesFile`: `.claude/`,
`CLAUDE.md`, `CLAUDE.local.md`, `AGENTS.md`, `.agents/`, `.cursor/`,
`.scratch/`, `.e2e/`, `.amx/`, `.mcp.json`, `opencode.json`,
`opencode.jsonc`, `skills-lock.json`, `.playwright-cli/` and any
`agent-memory` directory. The dotfiles install it. `workflow doctor` reports
an entry the file lacks and never writes it, because the dotfiles would undo
the edit on their next apply.

Plans carry their decisions as sentences, each with its reason.
`workflow plan-check` refuses a numbered list under a Rulings or Decisions
heading: a number gets cited in code and commit messages, where a reader of
the repository has no list to look it up in, and the reason is what should
land there instead.

## Which projects see the skills

A project mem knows. That is the whole answer, and it is the same answer to
"which projects does mem speak in".

`workflow skill` lists the ten skills this binary carries, one
`name — description` line each; `workflow skill route` prints that one whole;
`mem skill mem` does the same for the other, the skill about mem. `mem context`
names them in the digest it injects at the start of a session, so a session
learns which skills exist, and what each is for, exactly where mem has a
project to talk about.

Outside such a project — a checkout mem has never been told about, or a
directory that is not a checkout at all — mem says nothing and the skills go
unnamed. Registering a project is how you opt in:

    mem log "first note"        # a write registers this checkout

`workflow doctor --fix` installs all eleven as files, each SKILL.md verbatim
under `~/.claude/skills/<name>/` and `~/.agents/skills/<name>/`, because Claude
Code, pi, codex and opencode list the skills they find there. The binary is
the only source: `workflow doctor` reports a copy that differs from it or is a
symlink, and `--fix` writes the binary's text over it.

## Daily use

Three sizes of work, three moves:

- **Small change** — just ask a session for it. Route makes it a one-shot:
  the change, `workflow verify`, one commit, one `mem log` line. No ceremony.
- **Feature** — say "plan this". Answer one round of questions, approve the
  plan once, and let the session build it task by task. Measured
  2026-09-15: one strong session landed three milestones in the time a run
  spent on one task's fix rounds, so a run is for the case below only.
- **A whole plan in parallel** — for a plan whose wide wave holds three or
  more tasks that share no files, on a machine that carries that many
  workers (up to five), with workers on the strongest model: tell
  a session "run the <plan-id> plan". It starts `workflow run`, reads `workflow status`, decides retries
  and cleanup itself, and asks you only what is genuinely yours. Workers are
  amx agents in tmux panes, listed by `amx ls` and watched with
  `amx attach <id>`. Workers run
  on opus unless the project says otherwise (`mem project set model sonnet`)
  or one run does (`WORKFLOW_MODEL=sonnet workflow run`). A cheaper model
  can be given more reasoning: `mem project set effort max` starts every
  worker with `--effort max`, `unset` takes it back, and `WORKFLOW_EFFORT`
  does it for one run (empty means no flag). A task merges when the gate's
  suite is green on integration, and a task with a `Show:` line also needs
  evidence its worker filed since its dispatch,
  `mem evidence add --task <id> <file> --note "<what it shows>"`, keyed by
  the bare task id; an older capture does not count. The run ticks each task in its plan as it lands
  and leaves a milestone's roadmap tick to serve, which ticks it after its
  Show path walk passed. The session that owns the run waits with
  `workflow wait`, which returns the moment a question, a failure or the
  end needs it.

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

## Serve: a roadmap run with nobody watching

`workflow serve` runs every approved roadmap on this machine milestone after
milestone. Every few seconds it looks at each project with a checkout here
whose runner is this machine, nobody, or a machine whose claim went stale. A
project with no run going gets a pickup lead, plan-check, then one child
`workflow run` in its checkout. When every task lands, a `dogfood` session
walks the milestone's Show path, its steps cut at the commas and
semicolons of the `Show:` line, and only a passed walk ticks the milestone, writes the status and
handoff lines and a hygiene count, and goes on to the next one; with none
left the roadmap goes to `maintenance`. A `workflow run` started by hand
lands the plan and leaves the tick to serve, which walks that milestone on
its next tick. A run that stops short leaves the project `waiting` until its
plan, an answer or a request in its run dir changes. A worker's question, a
task's second failure and a failed walk get a lead session of their own.

The walk files a finding for every step it could not pass. A lead turns
the findings into fix tasks on the milestone's plan, the run lands them,
and the next walk covers only the steps that failed. The third failed walk
of a milestone pauses the project and asks you once; answering `walk again`,
or `workflow resume`, clears the pause and the count. In maintenance serve takes the oldest open
finding, has a lead store a fix plan for it, runs the plan and walks the
step again. A fix plan of three tasks or fewer that touches no path
`workflow review-needed` calls risky runs unasked; a bigger one waits for
your `run` or `hold`.

    workflow dogfood [<project>] [--milestone <slug>]

asks for a landed milestone to be walked again, the roadmap's last ticked
one unless named, at the checkout's head. It is a question for the engine,
and serve on the project's `dogfood-machine` (`mem project set
dogfood-machine <host>`, else this machine) walks it and answers with the
result. Milestone end asks that machine the same way when it is another
one. The engine never fetches or pushes: when the commit is not on that
machine the walk files a finding saying so, and the push is yours.

    workflow doctor --fix                        # writes the workflow.service unit
    systemctl --user enable --now workflow.service
    workflow serve --once                        # one tick by hand, to see what it does

`doctor --fix` writes the unit into the dotfiles' unit directory and links it
into `~/.config/systemd/user`, or writes it there when there are no dotfiles.
Enabling it is yours to do; doctor names the command.

Three controls:

- `workflow park <task> "<reason>"`: a lead labels a task whose question
  went to you. Serve starts no second lead for it, and the label goes once
  every question the task asked is answered.
- `workflow pause [<project>]`: serve stops the project's run, which stops
  its workers and leaves their tasks dispatched, and starts nothing.
- `workflow resume [<project>]`: serve starts the run again on the next
  tick, and the run adopts what the stop left.

`workflow status` opens with the serve line, and `workflow status --json`
carries the same fields: `stage` (pickup, execution, waiting, blocked-plan,
paused, dogfood, needs-plan, maintenance; where serve has written none,
execution for a live run and idle otherwise; needs-plan is a milestone with
no stored plan, which nothing starts until the plan skill stores one), `milestone` (slug and its place in the roadmap),
`parked` (task and reason), `findings` (open findings), `runner` (the machine
that holds the project) and `paused`.

SIGTERM or SIGINT to serve goes to every child run, which stops its workers
and leaves the tasks dispatched; serve waits up to thirty seconds for each and
exits 0. On start serve stops the lead sessions it recorded, and a run that
went without finishing its loop, stopped or killed, starts again on the next
tick and adopts its tasks, settling a merge a crash interrupted.
`tests/t131-soak.sh` proves it in two minutes in the suite; run it for twenty,
`WF_SOAK_MIN=20 bash tests/run.sh t131`, once before a milestone that changes
serve is ticked.

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

## The hub

`hub` is the phone's view of every project: a page per part of the
lifecycle, each 390 px wide with no script, read through `mem ... --json` and
`workflow status --json`. `hub.service` runs it on loopback and `tailscale
serve` publishes it on the tailnet.

- `/`: every project with its stage, runner, progress, the questions waiting
  on you and its last activity, then the sibling hubs.
- `/p/<project>`: the stage's front page, its progress and its buttons.
- `/p/<project>/roadmap`: the milestones with their Show paths and plans, and
  Approve and Request changes while the roadmap is a draft.
- `/p/<project>/run`: the task board, the live workers and the run log.
- `/p/<project>/questions`: each question with its options as buttons, the
  recommended one marked, a text box, and the answered ones with their answers.
- `/p/<project>/evidence`: the gallery by task, 24 a page, and the findings
  open and fixed. An image's bytes come through `mem evidence cat <id>`, so
  the hub never reads the store or a path a request names.
- `/p/<project>/wiki` and `/wiki/<project>/<slug>`: the pages, one page by
  section with its contents, and a search whose section hits open at the
  heading.
- `/p/<project>/decisions`: the rulings, who took each and what was
  recommended.
- `/p/<project>/new`: the forms.

A button or a form writes mem and nothing else, naming its project with
`--project=<name>`. The engine reads the write on its next tick, and the page
says `sent, waiting for the engine` until mem and `workflow status --json`
show it taken.

| Button or form | Writes |
|---|---|
| Approve | `mem roadmap --status approved`, then answers the pending `Approve roadmap` question `approve`. When another machine's runner holds the project, mem refuses, the hub writes nothing else and links that machine's hub. |
| Request changes | answers that question `changes: <text>`, or logs `roadmap changes requested: <text>` when none waits |
| Pause | `mem project set paused "<machine> <date>"`, the words `workflow pause` writes |
| Resume | `mem project unset paused`; a project paused after its third failed walk is resumed by answering its question `walk again` |
| A question's option or text box | `mem answer <id> <text>` |
| Idea | `mem idea <text>` |
| Brief | `mem brief --set <text>` |
| Finding | `mem finding add --milestone <slug> --step <n> --evidence <photo> <text>`, the photo a JPEG, PNG or WebP of 8 MiB at most |
| Ask for research, Ask for a research round | a question for the engine, `research on <machine>` or `research round on <machine>` |

The doorbell rings ntfy for a question waiting on you, a roadmap's approval
among them, and three more: a walk that ended (`walk passed` or `walk found
defects`, from serve's `dogfood <slug>:` run lines), the engine pausing a
project after its third failed walk, and a roadmap finished on this machine.
Each rings once, from the machine that wrote it, and carries the machine, the
project and the link, never the text.

    bash hub/tests/sandbox.sh                    # the hub from this checkout over a seeded throwaway store
    bash hub/tests/sandbox.sh shot / home.png    # one 390 px screenshot of a page, then stop

The sandbox prints `sandbox http://127.0.0.1:<port>/` and serves four projects,
one per stage, until killed, then removes its store. `shot` takes the
machine-wide lock `tests/t150-hub-browser.sh` takes to walk the Show path in a
browser, because one browser at a time is what this machine's memory holds.

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
3. Do the three-step start above (register, AGENTS.md, verifier).
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
