# workflow

Three binaries that run a solo development workflow, plus the thin surfaces
that carry it into an editor session.

- `mem/` is the system of record for project state: durable facts, rulings,
  logs, handoffs, blocking questions and a wiki of design pages, kept as
  markdown outside every project repo and synced between machines.
- `workflow/` is the gate and the orchestrator: `verify`, `lint-msg`,
  `review-needed`, plan-driven `run`, `status`, `reap`,
  `enable`/`disable`, `doctor`, and the body of the git hook stubs.
- `hub/` is a small web view over mem's question queue, served tailnet-only,
  so an open question can be answered from a phone.
- `skills/` holds the session-facing instructions (route, plan, implement,
  review, orchestrate, mem, unslop). `hooks/` holds the three git hook
  stubs. Another runtime joins by reading the same skills and exporting
  `WORKFLOW_AGENT=1` in its sessions; that wiring is per-runtime and kept
  outside this repo.

Install each binary with `cargo install --path <crate>`. Tests are
`cargo test` per crate plus `bash tests/run.sh`. `workflow doctor` and
`mem doctor` check a machine's wiring.

## Starting a project (new or existing)

The gate and the hooks are global; a project only needs three things, none
of them committed:

    cd ~/Sites/github/thing
    mem log "picking this up"        # first write registers the project

    # the four route lines, kept out of git for good:
    printf 'CLAUDE.md\n.claude/\n' >> .git/info/exclude
    $EDITOR CLAUDE.md                # copy the block from any other project

    mem project set verify "pnpm test"          # what green means here
    mem project set review-paths "scripts/**"   # extra risky globs, optional

From then on every session in that directory gets the project's memory at
start, the commit gate is armed, and the routing skill decides lanes.

A monorepo holds one project per app beside the root project:

    mem project add apps/thing               # a child project at that subdir
    cd apps/thing && mem project set verify "pnpm test --filter=thing"

Sessions inside `apps/thing` resolve to the child — its own plan, status,
handoff, wiki and questions — and everything else in the checkout stays with
the root. The pre-commit gate runs at the toplevel, so it always answers to
the root project's verify.

## Which projects see the skills

`workflow enable` and `workflow disable` write one key, `skillOverrides`,
naming route, plan, implement, orchestrate, review, mem and unslop one by one.
`--global` writes the user's settings file; without it, `.claude/settings.json`
at the repo's toplevel, which outranks the user's. So the switch runs either
way round:

    workflow enable --global     # the skills are there by default
    workflow disable             # except in this project

    workflow disable --global    # or: nowhere by default
    workflow enable              # except in this project

Settings are the only lever. Claude Code builds the skill list from its
settings files, the frontmatter switch is global to the skill, and no
environment variable is consulted for it.

## Daily use

Three sizes of work, three moves:

- **Small change** — just ask a session for it. Route makes it a one-shot:
  implement, `workflow verify`, one commit, one `mem log` line. No ceremony.
- **Feature** — say "plan this". Answer one round of questions, approve the
  plan once, and either let the session build it task by task or hand the
  plan to a run.
- **A whole plan in parallel** — tell a session "orchestrate the <plan-id>
  run". It starts `workflow run`, reads `workflow status`, decides retries
  and cleanup itself, and asks you only what is genuinely yours. Workers are
  background claude sessions by default: watch them in `claude agents`,
  attach to any. `mem project set backend amx` puts a project's workers in
  tmux panes instead, listed by `amx ls` and watched with `amx attach <id>`;
  waves, ownership and the merge gate are the same either way. Workers run
  on opus unless the project says otherwise (`mem project set model sonnet`)
  or one run does (`WORKFLOW_MODEL=sonnet workflow run`).

Questions find you: on screen while a machine is watched, on the phone
(ntfy via hub) when everything is locked. Answer in the session, with
`mem answer <id> "..."`, or from the hub page. A worker's question never
gets that far. Asked from a task worktree it is addressed to the
orchestrator, which answers it from the plan and the code, and the run
dispatches the task again with the answer in its brief. What reaches you is
only what the orchestrator could not settle, asked fresh in its own words.

## Reading a project's state

    mem wiki                     # the project's pages, one line each
    mem plan                     # the active plan, [x] ticks are progress
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
3. Do the three-step start above (register, CLAUDE.md via info/exclude,
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
