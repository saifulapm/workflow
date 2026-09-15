---
name: route
description: Use at the start of any code change, before touching a file, to decide whether it is a one-shot fix or wants a plan.
---

# route

Two lanes. Pick one, then stop thinking about lanes.

## First, reversibility

Could this be undone with a `git revert` and nothing else? If not — it touches
data, a public contract, a deployed thing, someone else's system — that is a
stop condition, not a lane, and it comes first: ask, do not choose.

## Then, blast radius

Zero blast radius means all five hold: no unintended consequence elsewhere ·
the intent needs no questions asked · no architectural decision · no new
dependency · no public contract changed (API, schema, signature, CLI).

Uncertain about any of them means not zero. Uncertainty fails upward, into the
plan lane, because that is the cheap direction to be wrong in.

## The deterministic disqualifier

    workflow review-needed

Exit 0 — the change touches auth, payments, secrets, migrations, jobs,
manifests, deploy config or a public API — means blast radius is not zero,
however small the diff looks. Exit 1 leaves the judgement above standing.

## Zero → the one-shot lane

Implement it, then `workflow verify` (0 green · 1 failed · 2 no verifier ·
3 test removal). Past sixty lines, or a `review-needed` diff, `workflow
read` runs first; under that the suite is the floor. Commit in ordinary
engineering voice, staging only the files this change touched. One `mem
log` line: what changed, and why. No plan, no subagents, no questions.

## Non-zero → the plan lane

Hand over to `plan`, or to `roadmap` when the work is more than one plan
holds. Do not write code first "to see how it goes": that is how a plan-lane
change becomes an unreviewed one-shot.

## What the gate is

A floor under ordinary forgetfulness. `--no-verify` and `env -u
WORKFLOW_AGENT` both walk around it, on the record. Not a sandbox.
