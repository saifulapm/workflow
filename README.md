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

Tests are `cargo test` in each crate and `bash tests/run.sh` for the hooks
and the hygiene check. The project's `verify` key runs all of them.

## Starting a project

    cd ~/Sites/github/thing
    mem log "picking this up"                  # the first write registers it
    mem project set verify "pnpm test"         # what green means here

Then run the grill skill for something new, or the plan skill when the
spec is settled. A monorepo holds one project per app beside the root
project: `mem project add apps/thing`.

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
