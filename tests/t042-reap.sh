#!/usr/bin/env bash
# workflow reap on its own: a run that ended without collecting its workers is
# picked up by the next reap, and reap's exit code says whether it did anything.
source "$(dirname -- "$0")/lib.sh"
t_init

new_repo reapme
mem_register
printf '{"name":"acme/app"}\n' >composer.json
printf '#!/bin/sh\nexit 0\n' >artisan
chmod +x artisan
write_exec bin/php <<-'EOF'
	#!/bin/sh
	exit 0
EOF
git add -A
git -c core.hooksPath=/dev/null commit -qm 'project files'
base=$(git rev-parse HEAD)
main=$PWD

run workflow reap
is "$RC" 0 'nothing running: reap has nothing to collect'
like "$OUT" 'nothing to collect' 'and says so'

# Hand-build what a run leaves behind when it is interrupted: a task whose
# worker has finished and exited but whose result nobody has looked at.
rundir="$XDG_STATE_HOME/workflow/runs/reapme/manual"
wtroot="$XDG_STATE_HOME/workflow/worktrees/reapme/manual"
mkdir -p "$rundir"
cat >"$rundir/plan.md" <<'EOF'
# plan: manual

- [ ] t1 Add the thing
      Files: app/**
      Verify: true
- [ ] t2 Never dispatched
      Files: other/**
      Verify: true
EOF
printf '%s\n' "$base" >"$rundir/base_sha"

git branch integration/manual "$base"
git worktree add -q "$wtroot/_integration" integration/manual
git worktree add -q -b manual/t1 "$wtroot/t1" "$base"
(
	cd "$wtroot/t1" || exit 1
	mkdir -p app
	printf '<?php\n' >app/Thing.php
	git add app/Thing.php
	git -c core.hooksPath=/dev/null commit -qm 'Add the thing'
)

printf '{"is_error":false,"result":"ok"}\n' >"$rundir/t1.json"
printf '2026-08-19T00:00:00Z ready merge-ready\n' >"$rundir/t1.status"

# The session's own transcript, where the context it was carrying is legible.
slug=$(printf '%s' "$wtroot/t1" | tr -c '[:alnum:]' '-')
mkdir -p "$HOME/.claude/projects/$slug"
cat >"$HOME/.claude/projects/$slug/00000000-0000-4000-8000-000000000000.jsonl" <<'EOF'
{"type":"assistant","message":{"usage":{"input_tokens":4,"cache_creation_input_tokens":900,"cache_read_input_tokens":12000}}}
{"type":"assistant","message":{"usage":{"input_tokens":2,"cache_creation_input_tokens":1500,"cache_read_input_tokens":157000}}}
EOF
printf '%s\n' "$(sh -c 'echo $$')" >"$rundir/t1.pid" # a pid that has already gone
printf '1\n' >"$rundir/t1.dispatches"
printf '00000000-0000-4000-8000-000000000000\n' >"$rundir/t1.session"
printf 'dispatched\n' >"$rundir/t1.state"
printf 'pending\n' >"$rundir/t2.state"

cd "$main" || exit 1
run workflow reap
is "$RC" 1 'reap collects the finished worker and says it did something'
is "$(cat "$rundir/t1.state")" merged 'the finished task went through the merge gate'
is "$(git rev-list --count "$base..integration/manual")" 1 'and landed on the integration branch'
is "$(cat "$rundir/t1.context" 2>/dev/null)" '158502' \
	'and the context it was carrying at its last turn was recorded'

run workflow reap
is "$RC" 0 'a second reap has nothing left to do'

git worktree remove --force "$wtroot/t1" 2>/dev/null
git worktree remove --force "$wtroot/_integration" 2>/dev/null

## ------------------------------------------- a merge interrupted mid-flight

# The fast-forward and the verify are two steps. A coordinator killed between
# them leaves integration advanced with the task still reading dispatched, and
# the pass that picks it up used to rebase commits integration already had and
# call the result a conflict (friction #DM877DNV).
rundir="$XDG_STATE_HOME/workflow/runs/reapme/halfway"
wtroot="$XDG_STATE_HOME/workflow/worktrees/reapme/halfway"
mkdir -p "$rundir"
cat >"$rundir/plan.md" <<'EOF'
# plan: halfway

- [ ] t1 Add the halfway thing
      Files: app/**
      Verify: true
EOF
printf '%s\n' "$base" >"$rundir/base_sha"

git branch integration/halfway "$base"
git worktree add -q "$wtroot/_integration" integration/halfway
git worktree add -q -b halfway/t1 "$wtroot/t1" "$base"
(
	cd "$wtroot/t1" || exit 1
	mkdir -p app
	printf '<?php\n' >app/Halfway.php
	git add app/Halfway.php
	git -c core.hooksPath=/dev/null commit -qm 'Add the halfway thing'
)
# The fast-forward that landed, and the intent line that says it was going to.
landed=$(git rev-parse halfway/t1)
(
	cd "$wtroot/_integration" || exit 1
	git merge -q --ff-only "$landed"
)
printf '%s %s\n' "$base" "$landed" >"$rundir/t1.merging"

printf '{"is_error":false,"result":"ok"}\n' >"$rundir/t1.json"
printf '2026-08-19T00:00:00Z ready merge-ready\n' >"$rundir/t1.status"
printf '%s\n' "$(sh -c 'echo $$')" >"$rundir/t1.pid"
printf '1\n' >"$rundir/t1.dispatches"
printf '00000000-0000-4000-8000-000000000002\n' >"$rundir/t1.session"
printf 'dispatched\n' >"$rundir/t1.state"

cd "$main" || exit 1
run workflow reap
is "$(cat "$rundir/t1.state")" merged 'an already-applied merge is finished, not re-rebased'
unlike "$(cat "$rundir/t1.parked" 2>/dev/null)" 'conflicts' \
	'and it is never mis-parked as a conflict with integration'
is "$(git rev-parse integration/halfway)" "$landed" \
	'integration keeps the commit that was already on it'
is "$(git rev-parse "refs/workflow/halfway/t1")" "$landed" \
	'and the commit that landed is recorded for a later run'
is "$(cat "$rundir/t1.merging")" '' 'the intent line is cleared once the merge is settled'

git worktree remove --force "$wtroot/t1" 2>/dev/null
git worktree remove --force "$wtroot/_integration" 2>/dev/null

## ------------------------------------------------- the three liveness signals

# Each of the three can go quiet on a worker that is perfectly fine, so the
# deadline measures the latest of them (review-3 F-10). `workflow stalled` is
# the seam that asks the liveness rule one signal at a time.
RUN_DIR="$T_TMP/liverun"
RUN_WT_ROOT="$T_TMP/livewt"
session=00000000-0000-4000-8000-000000000001
worker_stalled() { workflow stalled --rundir "$RUN_DIR" --wtroot "$RUN_WT_ROOT" --deadline 5 "$1"; }

mkdir -p "$RUN_DIR" "$RUN_WT_ROOT/t1"
printf '%s\n' "$session" >"$RUN_DIR/t1.session"
old=$(($(date +%s) - 3600))
touch -d "@$old" "$RUN_WT_ROOT/t1"
printf '%s\n' "$old" >"$RUN_DIR/t1.dispatched_at"
: >"$RUN_DIR/t1.status"
touch -d "@$old" "$RUN_DIR/t1.status"
run worker_stalled t1
is "$RC" 0 'all three signals cold: the worker is stalled'

touch "$RUN_DIR/t1.status"
run worker_stalled t1
isnt "$RC" 0 'a fresh heartbeat line in the status file keeps it alive'

touch -d "@$old" "$RUN_DIR/t1.status"
# The transcript of a print-mode worker: the worktree path with every
# non-alphanumeric turned into a dash.
slug=$(printf '%s' "$RUN_WT_ROOT/t1" | sed 's/[^A-Za-z0-9]/-/g')
tp="$HOME/.claude/projects/$slug/$session.jsonl"
mkdir -p "$(dirname -- "$tp")"
touch "$tp"
run worker_stalled t1
isnt "$RC" 0 'a fresh transcript line keeps it alive'

rm -f "$tp"
touch "$RUN_WT_ROOT/t1/scratch"
run worker_stalled t1
isnt "$RC" 0 'and so does a fresh write in the worktree'

## --------------------------------------------- a second run of the same plan

new_repo again
mem_register
cat >"$T_TMP/again.md" <<'PLAN'
# plan: again

- [ ] t1 One
      Files: a/**
      Verify: true
- [ ] t2 Two
      Files: b/**
      Verify: true
PLAN
git branch again/t1
run workflow run --plan-file "$T_TMP/again.md"
is "$RC" 2 'a leftover task branch stops the run before it dispatches anything'
like "$OUT" 'still here from an earlier run' 'and says where it came from'
like "$OUT" 'git branch -D again/t1' 'and what to type if it is not wanted'
is "$(git worktree list | grep -c .)" 1 'nothing was created in the meantime'

## -------------------------------------------------- a dispatch that never was

# A template that writes no pidfile and no result: the worker is not slow, it
# never started. The park reason has to say that rather than blame a worker
# that does not exist.
new_repo racy
mem_register
printf '{"name":"acme/r"}\n' >composer.json
printf '#!/bin/sh\nexit 0\n' >artisan
chmod +x artisan
write_exec bin/php <<-'PHP'
	#!/bin/sh
	exit 0
PHP
git add -A
git -c core.hooksPath=/dev/null commit -qm 'project files'
cat >"$T_TMP/racy.md" <<'PLAN'
# plan: racy

- [ ] t1 Never gets going
      Files: a/**
      Verify: true
- [ ] t2 Nor does this one
      Files: b/**
      Verify: true
PLAN
run env WORKFLOW_MAX_WORKERS=2 WORKFLOW_DEADLINE_MIN=0.5 \
	WORKFLOW_WORKER_CMD='true' workflow run --plan-file "$T_TMP/racy.md"
is "$RC" 1 'a dispatch that produced nothing parks the task'
racy="$XDG_STATE_HOME/workflow/runs/racy/racy"
is "$(cat "$racy/t1.parked")" 'dispatch race: worker never wrote its pidfile' \
	'and names the dispatch, not a worker that never ran'
