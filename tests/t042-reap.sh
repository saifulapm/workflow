#!/usr/bin/env bash
# workflow reap on its own: a run that ended without collecting its workers is
# picked up by the next reap, and reap's exit code says whether it did anything.
source "$(dirname -- "$0")/lib.sh"
t_init

# Every worker here is fabricated in the process seam's shape -- a pidfile, a
# result document, a transcript -- so reap and run read them through that
# seam; the sections that dispatch set their own template over this one.
export WORKFLOW_WORKER_CMD=true

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
unlike "$(cat "$rundir/t1.failed" 2>/dev/null)" 'conflicts' \
	'and it is never mis-failed as a conflict with integration'
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
# The leftover holds a commit, so it is adopted rather than refused over
# (m1-lessons ruling 11; t055 walks the adoption to its merge). An empty one
# would be swept (friction #1916K336, covered in t044).
git branch again/t1 "$(git commit-tree 'HEAD^{tree}' -p HEAD -m 'leftover work')"
run workflow run --plan-file "$T_TMP/again.md"
like "$OUT" 't1: left by an earlier run with 1 commit\(s\) -- adopted on its branch' \
	'a leftover task branch with commits is adopted, for the gate and the reader to judge'
unlike "$OUT" 'still here from an earlier run' 'never the merge-or-delete recipe that was merged by hand'
is "$RC" 2 'this run still stops -- on the red trunk behind it, not on the leftover'
is "$(cat "$XDG_STATE_HOME/workflow/runs/again/again/t1.dispatches" 2>/dev/null)" '' \
	'and nothing was dispatched in the meantime'

## -------------------------------------------------- a dispatch that never was

# A template that writes no pidfile and no result: the worker is not slow, it
# never started. The failure reason has to say that rather than blame a worker
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
is "$RC" 1 'a dispatch that produced nothing fails the task'
racy="$XDG_STATE_HOME/workflow/runs/racy/racy"
is "$(cat "$racy/t1.failed")" 'dispatch race: the worker never started' \
	'and names the dispatch, not a worker that never ran'
is "$(cat "$racy/model" 2>/dev/null)" opus \
	'setup records the model this run dispatches on'
is "$(cat "$racy/review-model" 2>/dev/null)" '' \
	'and an empty review-model when nobody reads'

## ------------------------- workers still running for a run that is gone

# Killing a coordinator does not take its workers down with it, and some of
# them can be working tasks a later pass already settled: rebuilding merged
# work with nothing left to gate them (friction #RF50DJXQ). reap stops those,
# and for a live worker on a still-dispatched task it says a run can adopt it
# rather than claiming there is nothing to collect.
new_repo orphaned
mem_register
obase=$(git rev-parse HEAD)
orundir="$XDG_STATE_HOME/workflow/runs/orphaned/ghosts"
mkdir -p "$orundir"
cat >"$orundir/plan.md" <<'PLAN'
# plan: ghosts

- [ ] t1 Already settled
      Files: a/**
      Verify: true
- [ ] t2 Still going
      Files: b/**
      Verify: true
PLAN
printf '%s\n' "$obase" >"$orundir/base_sha"
# The stand-in workers close every inherited fd: a child holding the runner's
# pipe open would stall the whole suite on this file.
setsid sleep 300 >/dev/null 2>&1 &
opid=$!
printf '%s\n' "$opid" >"$orundir/t1.pid"
printf '00000000-0000-4000-8000-00000000000a\n' >"$orundir/t1.session"
printf 'merged\n' >"$orundir/t1.state"
printf '2\n' >"$orundir/t1.dispatches"
setsid sleep 300 >/dev/null 2>&1 &
opid2=$!
printf '%s\n' "$opid2" >"$orundir/t2.pid"
printf '00000000-0000-4000-8000-00000000000b\n' >"$orundir/t2.session"
printf 'dispatched\n' >"$orundir/t2.state"
printf '%s\n' "$(date +%s)" >"$orundir/t2.dispatched_at"
printf '1\n' >"$orundir/t2.dispatches"

run workflow reap
is "$RC" 1 'reap collected something'
like "$OUT" 't1: merged already' 'the settled task with a live worker is named'
for _ in 1 2 3 4 5 6 7 8 9 10; do kill -0 "$opid" 2>/dev/null || break; sleep 0.2; done
is "$(kill -0 "$opid" 2>/dev/null && echo alive || echo gone)" gone \
	'and its worker was stopped'
is "$(kill -0 "$opid2" 2>/dev/null && echo alive || echo gone)" alive \
	'the worker on a still-dispatched task is left for adoption'
like "$OUT" 'adopt' 'and reap says a run can adopt it, not that there is nothing to collect'
unlike "$OUT" 'nothing to collect' 'the nothing-to-collect line stays honest'
kill "$opid" "$opid2" 2>/dev/null

## ----------------------- a worker that died leaving nothing, and no run

# The one retry a task nobody has really attempted gets is a run's to give:
# a run watches what it starts. reap collects for a run that is gone, and it
# used to start a worker that nothing would watch (friction #F6MR6AMH).
new_repo silent
mem_register
sbase=$(git rev-parse HEAD)
srundir="$XDG_STATE_HOME/workflow/runs/silent/quiet"
mkdir -p "$srundir"
cat >"$srundir/plan.md" <<'PLAN'
# plan: quiet

- [ ] t1 Died before a word
      Files: a/**
      Verify: true
- [ ] t2 Never dispatched
      Files: b/**
      Verify: true
PLAN
printf '%s\n' "$sbase" >"$srundir/base_sha"
printf '%s\n' "$(sh -c 'echo $$')" >"$srundir/t1.pid" # a pid that has already gone
printf '00000000-0000-4000-8000-00000000000c\n' >"$srundir/t1.session"
printf 'dispatched\n' >"$srundir/t1.state"
printf '%s\n' "$(date +%s)" >"$srundir/t1.dispatched_at"
printf '1\n' >"$srundir/t1.dispatches"
printf 'pending\n' >"$srundir/t2.state"

run env WORKFLOW_WORKER_CMD="touch $T_TMP/reap-dispatched" workflow reap
is "$RC" 1 'reap collected the silent task'
is "$(cat "$srundir/t1.state")" failed 'a worker that died leaving nothing is failed, not tried again'
like "$(cat "$srundir/t1.failed")" 'next run' 'and the failure names who tries it next'
is "$(cat "$srundir/t1.dispatches")" 1 'reap dispatched nothing'
truthy "$([ ! -e "$T_TMP/reap-dispatched" ] && echo 0 || echo 1)" \
	'and started no worker for a run nobody is watching'
unlike "$OUT" 'one more try' 'nor did it promise one'

## ---------------------------------------- reap starts no reader of its own

# reap collects and never dispatches, and a reader is a dispatch: started
# under reap it would have no run to judge it, and `workflow reap` run from
# a checkout with a plan mid-gate spawned one anyway (friction #NNVWGXZ4).
# The setup is a run killed with a finished worker nobody has gated yet --
# the one shape where reap reaches the gate at all.
new_repo modelcheck
mem_register
printf '{"name":"acme/models"}\n' >composer.json
printf '#!/bin/sh\nexit 0\n' >artisan
chmod +x artisan
write_exec bin/php <<-'EOF'
	#!/bin/sh
	exit 0
EOF
git add -A
git -c core.hooksPath=/dev/null commit -qm 'project files'
cat >"$T_TMP/models.md" <<'PLAN'
# plan: models

- [ ] t1 Add the thing
      Files: app/**
      Verify: true
- [ ] t2 Never dispatched
      Files: other/**
      Verify: true
PLAN
# A real run, the way the racy run above makes one, so the review model reap
# reads below is one setup actually wrote, not a printf standing in for it.
run env WORKFLOW_MAX_WORKERS=2 WORKFLOW_DEADLINE_MIN=0.5 WORKFLOW_WORKER_CMD='true' \
	WORKFLOW_REVIEW_MODEL=fake-reader workflow run --plan-file "$T_TMP/models.md"
mrundir="$XDG_STATE_HOME/workflow/runs/modelcheck/models"
mwtroot="$XDG_STATE_HOME/workflow/worktrees/modelcheck/models"
is "$(cat "$mrundir/review-model" 2>/dev/null)" fake-reader \
	'setup records the review model this run reads with'

# The race above failed both tasks and cleanup took their worktrees down;
# rebuild them on the branches setup already made, then hand-build the
# interruption: t1 finished and nobody has gated it, t2 was never dispatched.
git worktree add -q "$mwtroot/_integration" integration/models
git worktree add -q "$mwtroot/t1" models/t1
(
	cd "$mwtroot/t1" || exit 1
	mkdir -p app
	printf '<?php\n' >app/Thing.php
	git add app/Thing.php
	git -c core.hooksPath=/dev/null commit -qm 'Add the thing'
)
printf '{"is_error":false,"result":"ok"}\n' >"$mrundir/t1.json"
printf '2026-08-19T00:00:00Z ready merge-ready\n' >"$mrundir/t1.status"
printf '%s\n' "$(sh -c 'echo $$')" >"$mrundir/t1.pid" # a pid that has already gone
printf '1\n' >"$mrundir/t1.dispatches"
printf '00000000-0000-4000-8000-00000000000d\n' >"$mrundir/t1.session"
printf 'dispatched\n' >"$mrundir/t1.state"
rm -f "$mrundir/t1.failed" "$mrundir/t1.dispatched_at" \
	"$mrundir/t2.failed" "$mrundir/t2.dispatches" "$mrundir/t2.dispatched_at" \
	"$mrundir/t2.session" "$mrundir/t2.status"
printf 'pending\n' >"$mrundir/t2.state"

export WF_TMP="$T_TMP"
write_exec "$T_TMP/reviewer.sh" <<'FAKE'
#!/bin/sh
task=$1; brief=$5; model=$6
printf '%s %s\n' "$model" "$task" >>"$WF_TMP/reviews.log"
answer=$(sed -n 's/^    Answer file: //p' "$brief")
printf 'VERDICT: ship\n' >"$answer"
FAKE
export FAKE="$T_TMP/reviewer.sh"

unset WORKFLOW_REVIEW_MODEL
run env WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$FAKE" {task} {worktree} {status} {session} {brief} {model}'"'"' > {out} 2> {err} &' workflow reap
unset WORKFLOW_REVIEW_MODEL

is "$(cat "$T_TMP/reviews.log" 2>/dev/null)" '' \
	'reap started no reader, though the run it collects for records one'
truthy "$([ ! -e "$mrundir/t1.review-prompt" ] && echo 0 || echo 1)" \
	'and wrote no reading prompt'
truthy "$([ ! -e "$mrundir/t1.review-session" ] && echo 0 || echo 1)" \
	'nor a reader session to watch'

git worktree remove --force "$mwtroot/t1" 2>/dev/null
git worktree remove --force "$mwtroot/_integration" 2>/dev/null

## ------------------- reap stays quiet about a run with nothing left to read

# A finished run's directory is never removed, so every later reap keeps
# seeing it. Naming the model it would read with is only honest for a run
# that still has something dispatched; one with everything already settled
# has no reading ahead of it, and saying so anyway (review 2) claims a
# reading that never happens.
qrundir="$XDG_STATE_HOME/workflow/runs/modelcheck/settled"
mkdir -p "$qrundir"
cat >"$qrundir/plan.md" <<'PLAN'
# plan: settled

- [ ] t1 Already merged
      Files: a/**
      Verify: true
- [ ] t2 Already failed
      Files: b/**
      Verify: true
PLAN
git rev-parse HEAD >"$qrundir/base_sha"
printf 'fake-reader\n' >"$qrundir/review-model"
printf 'merged\n' >"$qrundir/t1.state"
printf 'failed\n' >"$qrundir/t2.state"

unset WORKFLOW_REVIEW_MODEL
run workflow reap
unset WORKFLOW_REVIEW_MODEL
unlike "$OUT" 'reading with' \
	'a settled run with nothing dispatched says nothing about reading'
