# workflow

Three binaries that run a solo development workflow, plus the thin surfaces
that carry it into an editor session.

- `mem/` is the system of record for project state: durable facts, rulings,
  logs, handoffs and blocking questions, kept as markdown outside every
  project repo and synced between machines.
- `workflow/` is the gate and the orchestrator: `verify`, `lint-msg`,
  `review-needed`, plan-driven `run`, `status`, `reap`, `park`/`resume`,
  `doctor`, and the body of the git hook stubs.
- `hub/` is a small web view over mem's question queue, served tailnet-only,
  so an open question can be answered from a phone.
- `skills/` holds the session-facing instructions (route, plan, implement,
  review, orchestrate, mem, unslop). `hooks/` holds the three git hook
  stubs. `adapters/` carry the same contract to other runtimes.

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
  background sessions: watch them in `claude agents`, attach to any.

Questions find you: on screen while a machine is watched, on the phone
(ntfy via hub) when everything is locked. Answer in the session, with
`mem answer <id> "..."`, or from the hub page.

## Reading a project's state

    mem plan                     # the active plan, [x] ticks are progress
    mem log                      # what happened, newest first
    mem log --kind ruling        # decisions taken instead of asking you
    mem search "friction"        # what is queued for the next version
    mem show <id>                # the full item behind any #id
    mem questions                # what is waiting on you
    mem handoff                  # where the last session stopped
    mem status                   # the standing one-paragraph status
    workflow status              # a live run: per-task states and reports

The concrete case: to see what amx v3 should fix, run
`mem search "friction" --project amx` — every dogfooding finding is there
with what happened and what was expected. `mem context` opens every session
with the same digest, so a fresh session already knows.

All of it works from anywhere with `--project <name>`, and from any machine:
the store syncs. `mem projects` is the portfolio view.

## Pausing, moving, finishing

- **Leaving a machine mid-work**: `workflow park` bundles the branch plus
  uncommitted changes and pushes the bundle to the sync hub immediately;
  `mem handoff --set "..."` says what is next. On the other machine,
  `workflow resume <bundle>` restores all of it. Orchestrated tasks that
  fail park themselves the same way.
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

    mem save --project workflow "friction: what - where - expected"

Product findings go against their own project the same way (that is where
the amx v3 queue came from). Nothing is fixed mid-task. When a few pile up,
say "batch review": one session verifies each claim against the code,
fixes what is real, declines the rest with a recorded ruling, and empties
the queue. Rulings you disagree with are cheap to overturn — that is what
they are for.
