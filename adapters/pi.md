# PI adapter

Paste this into PI's system prompt or per-project instruction block. PI has no
skills format either, so the same four skills arrive as prose — and the same
one line of shell is mandatory.

---

## Environment

First thing in every session, before any git command:

    export WORKFLOW_AGENT=1

The gate fires on a union: the checkout is one the orchestrator made, **or**
`WORKFLOW_AGENT` is set and mem knows this checkout. Working in an ordinary
checkout, only the second half can apply to you. If you skip the export, every
commit you make is ungated and nothing will tell you.

## Which lane

`workflow review-needed`. Exit 0 — auth, payments, secrets, migrations, jobs,
manifests, deploy configuration, public API — means plan first, and get the
plan approved before code. Exit 1 with clear intent, no new dependency, no
architectural decision and no public contract change means it can go straight
in. If you are unsure, it is the plan lane; uncertainty fails upward.

## Every change

1. Failing test, then the code.
2. `workflow verify` — 0 green · 1 failed · 2 no verifier · 3 test removal.
   Exits 2 and 3 each print the exact `mem save --kind ruling …` command that
   records the decision. Run it if the decision is right; do not work around
   the check.
3. Commit in ordinary engineering voice, staging only what this change touched.
   `workflow lint-msg` refuses provenance trailers, "generated with" lines,
   session links, `focus:`/`gate:`/`track/` prefixes and robot or sparkle
   emoji, and the commit-msg hook runs it for you.
4. `mem log "<what landed>"`.

## Stop and ask

Irreversible · security-sensitive · anything outside this worktree (push,
publish, deploy, external write) · the plan is broken beyond guessing ·
credentials or secrets. `mem ask "<question>"`, `mem handoff --set "<state>"`,
then stop. Do not pick the safest-looking option and carry on.

The pre-push hook refuses pushes. With a human present who has approved it:
`WORKFLOW_ALLOW_PUSH=1 git push`.

## Recording decisions

    mem save --kind ruling --type <type> "<what - why - cost if wrong>"
