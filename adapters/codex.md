# Codex adapter

Paste this into the Codex instruction file for a repo (`AGENTS.md`, or the
project instructions field). Codex has no skills format, so the four skills
become one block of prose and one non-negotiable line of shell.

---

## Environment

At the start of every session, before any git command, run:

    export WORKFLOW_AGENT=1

Without it, nothing you commit in this checkout is gated. The pre-commit hook
fires on the union of two conditions: the checkout is one the orchestrator
made, **or** `WORKFLOW_AGENT` is set and mem knows this checkout. Only the
second one applies to you, and only if you export it. This is not optional and
it is not a preference; it is the whole gate for this runtime.

## Before touching a file

Run `workflow review-needed`. Exit 0 means the change touches auth, payments,
secrets, migrations, jobs, manifests, deploy configuration or a public API:
write a plan and get it approved before writing code. Exit 1 and a change with
no plausible path to unintended consequences elsewhere may go straight in.

## Every change

1. Failing test first, then the code that passes it.
2. `workflow verify` — 0 green · 1 failed · 2 no verifier · 3 test removal.
   Exit 2 prints the `mem save --kind ruling --type no-verifier …` command to
   record the decision; exit 3 prints the one for a removed test.
3. Commit. Ordinary engineering voice. No `Co-Authored-By`, no "generated
   with", no session links, no `focus:`/`gate:`/`track/` prefixes, no robot or
   sparkle emoji — `workflow lint-msg` refuses all of those and the commit-msg
   hook runs it. Stage only the files this change touched; never `git add -A`.
4. One `mem log "<what landed>"` line.

## Stop and ask — never decide these yourself

Irreversible change · security-sensitive change · anything that leaves this
worktree (push, publish, deploy, external write) · the plan is broken beyond
guessing · credentials or secrets. On any of them: `mem ask "<question>"`,
`mem handoff --set "<where you are>"`, stop.

Pushing is refused by the pre-push hook. With a human present who has said yes,
it is `WORKFLOW_ALLOW_PUSH=1 git push`, once, deliberately.

## Recording decisions

    mem save --kind ruling --type <type> "<what - why - cost if wrong>"

`mem log --kind ruling --since 8h` at the end of a session is what you show for
what you decided.
