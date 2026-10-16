# workflow

A solo development workflow in three binaries and a handful of skills.
An agent runs each milestone end to end, and everything it knows lives
outside the project repos.

- `mem/` is the system of record: facts, decisions, logs, handoffs,
  questions, findings, evidence, roadmaps, plans and a wiki of spec
  pages. It keeps markdown files outside every repo and syncs them between
  machines.
- `hub/` is a small web server over mem, served on the tailnet, so a phone
  can follow every project, read its plans and answer its questions.
- `workflow/` is the glue: the clean-repo check behind the git hooks,
  `install`, which writes the skills, subagents and hook stubs, and `go`,
  which starts a milestone's orchestrator.
- `skills/` and `agents/` hold the instructions the sessions follow.
  `hooks/` holds the three git hook stubs.

## The flow

1. **grill.** Saiful brings an idea. A session researches it, interviews
   him in rounds until no decision is open, and writes the spec as wiki
   pages in mem.
2. **plan.** The roadmap is a list of milestones, each a vertical slice
   with a Show path. Each milestone has a markdown plan written for the
   agents (behaviours with their acceptance, the decisions taken, scope),
   which links any claude.ai artifact he made for it. He approves the
   roadmap once, in the chat or on the hub.
3. **go.** `workflow go <project>` starts an Opus orchestrator in an amx
   pane with `/goal`, so Claude Code keeps it working until the milestone
   is landed. It builds test-first, alone or with worktree subagents, runs
   the whole suite once per wave, has one cold review and one dogfood walk,
   fixes what they find test-first, records its lessons, and then runs
   `workflow go` for the next milestone.

The orchestrator decides everything reversible and records each decision in
mem. It asks only about pushing, deploying, money, deleting data, messages to
people, security and taste with no default, and keeps working while it
waits. A dead orchestrator shows as stalled on the hub, and Resume starts a
fresh one from its handoff.

## Commands

    workflow go [<project>] [--milestone <slug>] [--model <m>] [--effort <l>] [--dry-run]
    workflow install
    workflow hygiene [--staged|--tree|--history <n>|--message <file>|--string <s>] [--fix]
    workflow lint-msg [<file>] [--string <text>]
    workflow hook <name> [--stub <path>] [-- <args>]

`install` writes every skill to `~/.claude/skills` and `~/.agents/skills`,
the subagents to `~/.claude/agents`, and the hook stubs to
`~/.config/git/hooks`, and removes what older versions installed. Run it
after every build.

Install the binaries into `~/.local/bin`, where every machine and
hub.service run them, then restart the hub. A plain `cargo install` would
put a second copy in `~/.cargo/bin`, which comes first on PATH.

    cargo install --force --path workflow --root ~/.local
    cargo install --force --path mem --root ~/.local
    cargo install --force --path hub --root ~/.local
    workflow install
    systemctl --user restart hub

On the mini, the weekly snapshot copied to Dropbox:

    install -m 755 mem/units/mem-snapshot.sh ~/.local/bin/mem-snapshot
    cp mem/units/mem-snapshot.service mem/units/mem-snapshot.timer ~/.config/systemd/user/
    systemctl --user enable --now mem-snapshot.timer

Tests are `cargo test` in each crate and `bash tests/run.sh` for the hooks
and the hygiene check. The project's `verify` key runs all of them.

## Starting a project

    cd ~/Sites/github/thing
    mem log "picking this up"                  # the first write registers it
    mem project set verify "pnpm test"         # what green means here

Then run the grill skill for something new, or the plan skill when the
spec is settled. A monorepo holds one project per app beside the root
project: `mem project add apps/thing`. A new app gets that project and a
worktree of its own before it is grilled, as the next section says.

## The clean-repo rule

A product repo reads as if a person wrote it. Every plan, note, log,
screenshot and decision goes to mem, never into the repo.
`workflow hygiene` enforces it in two tiers. The hard tier refuses a file on
the global ignore list tracked or staged, and, in added lines of
non-markdown files and in commit messages, a numbered ruling, milestone,
ticket, issue or ADR, a mem id, or a `Co-Authored-By` or `Generated with`
line. A message also fails on a subject over 72 characters and on a bare
task id. The soft tier only warns, on messages.

The pre-commit hook runs the hygiene check on the staged diff, and
commit-msg checks the message. Neither runs the test suite.
`WORKFLOW_HYGIENE=skip git commit` lets a person's commit through with a
warning. Nothing clears the hard tier for an agent. A project exempts test
fixtures with `mem project set hygiene-exempt "tests/**"`.

The hooks check only checkouts mem knows. pre-push refuses an agent's push
(`WORKFLOW_AGENT`, or pi's `PI_CODING_AGENT`) unless `WORKFLOW_ALLOW_PUSH=1`
is set, which a person sets when they say yes.

The global ignore list comes from the dotfiles through `core.excludesFile`:
`.claude/`, `CLAUDE.md`, `CLAUDE.local.md`, `AGENTS.md`, `.agents/`,
`.cursor/`, `.scratch/`, `.e2e/`, `.amx/`, `.mcp.json`, `opencode.json`,
`opencode.jsonc`, `skills-lock.json`, `.playwright-cli/` and any
`agent-memory` directory.

## Two apps of one monorepo at once

Each app whose milestones run at the same time works in a checkout of its
own, on its own trunk branch, and `main` is checked out nowhere. `workflow
go` refuses a second project's orchestrator in a checkout that already has
one, a checkout recorded for another project whether or not its
orchestrator runs, and a checkout off `main` unless it is the worktree
recorded for the project. None of these is a reason to switch a
checkout's branch: the branch is another app's trunk. Once `main` is
checked out nowhere, that holds for every app that runs milestones, not
only the ones running at the same time: each needs a recorded worktree,
the main folder included.

The app has to be a child project first (`mem project add apps/lareys`),
or `mem project worktree` records the folder for the root project:

    cd ~/Sites/github/shopify_apps
    git worktree add -b app/lareys ../shopify_apps-lareys main
    cd ../shopify_apps-lareys/apps/lareys
    mem project worktree .        # Start and Resume on the hub run here now

Moving an app that already runs in the main folder onto its own trunk is a
one-time step by hand. Do it between milestones, when `amx ls` shows no
orchestrator of that app running. For ShortCart:

    cd ~/Sites/github/shopify_apps
    git switch -c app/shortcart   # main is now checked out nowhere
    cd apps/shortcart && mem project worktree ~/Sites/github/shopify_apps

### Landing an app trunk on main

Land between milestones, in the app's checkout, on its trunk. Rebase onto
`main`, and when the rebase stops on `pnpm-lock.yaml`, regenerate it rather
than merging it by hand:

    git rebase main
    git checkout --ours pnpm-lock.yaml   # main's, mid-rebase
    pnpm install --lockfile-only
    git add pnpm-lock.yaml && git rebase --continue

Run the project's `verify`, then fast-forward `main` to the trunk. The
fetch refuses anything but a fast-forward, and refuses while `main` is
checked out in any folder:

    git fetch . app/lareys:main

The other apps rebase onto the new `main` when they next land. A change to
`packages/*` or to the workspace's version catalogs lands on `main` first,
on its own branch, before any app builds on it. Branch names carry the
app, as in `integration/<app>/<milestone>`. Releases tag from `main`.
