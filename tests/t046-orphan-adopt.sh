#!/usr/bin/env bash
# An orchestrator that died mid-run, and the run that picks up after it.
#
# The lock goes with the dead process's file descriptors, so the next
# invocation is free to start -- and what it finds is tasks still marked
# `dispatched` whose worker may or may not still be standing in the worktree.
# It must never dispatch a second worker into a worktree that has one, never
# hold a wave open for a worker that is gone, and never take a worktree down
# while somebody is working in it (friction #X84NTCDB).
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"

# The worker for the tasks this run really dispatches: commits the one file
# its task owns and reports ready.
write_exec "$T_TMP/fake-worker.sh" <<'FAKE'
#!/bin/sh
task=$1; status=$3; brief=$4
case $task in
*-review)
	printf '%s\n' "$task" >>"$WF_TMP/reviews.log"
	while [ -f "$WF_TMP/hold-review" ]; do sleep 0.2; done
	answer=$(sed -n 's/^    Answer file: //p' "$brief")
	printf 'VERDICT: ship\n' >"$answer"
	exit 0
	;;
esac
printf '%s started\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$status"
mkdir -p app
# Named for its plan too: a finished plan lands on the checkout, and the next
# plan's task over the same file needs a change to commit.
# And stamped per dispatch: this file runs the same plan again after it landed.
printf '%s %s %s\n' "$(basename "$(dirname "$PWD")")" "$task" "$(date +%s%N)" >"app/$task.php"
git add "app/$task.php"
git -c core.hooksPath=/dev/null commit -qm "Add the $task service"
printf '%s ready\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$status"
printf '{"is_error":false,"result":"ok"}\n'
FAKE

export FAKE="$T_TMP/fake-worker.sh"
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$FAKE" {task} {worktree} {status} {brief}'"'"' > {out} 2> {err} &'
export WORKFLOW_MAX_WORKERS=2 WORKFLOW_DEADLINE_MIN=0.2

new_repo app
mem_register
# A verifier, so the merge gate has a suite to be green about.
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
repo=$PWD
rundir="$XDG_STATE_HOME/workflow/runs/app/orphan-check"
wtroot="$XDG_STATE_HOME/workflow/worktrees/app/orphan-check"

plan='# plan: orphan-check

- [ ] t1 First service
      Files: app/t1.php
      Verify: true
- [ ] t2 Second service
      Files: app/t2.php
      Verify: true
'

# What a dead orchestrator leaves: the run dir mid-flight, the worktrees and
# branches it made still there, and t1 recorded as dispatched. Fabricated
# rather than raced, so the state under test is exactly the state described.
orphan() {
	mkdir -p "$rundir" "$wtroot"
	printf '%s\n' "$base" >"$rundir/base_sha"
	printf '%s' "$plan" >"$rundir/plan.md"
	# The plan file too: a run ticks its merges off in the file it was handed,
	# so each section here starts from the plan as its author wrote it.
	printf '%s' "$plan" >"$T_TMP/plan.md"
	printf 'dispatched\n' >"$rundir/t1.state"
	printf 'pending\n' >"$rundir/t2.state"
	printf '1\n' >"$rundir/t1.dispatches"
	printf '%s\n' "$(date -u +%s)" >"$rundir/t1.dispatched_at"
	printf 'orphan-session\n' >"$rundir/t1.session"
	git -C "$repo" worktree add -q -b orphan-check/t1 "$wtroot/t1" "$base"
	git -C "$repo" worktree add -q -b orphan-check/t2 "$wtroot/t2" "$base"
}

## ------------------------------------------- the worker died with the run

# The work t1 finished before its orchestrator went: a commit on its branch, a
# status file ending in ready, and the result document the worker left behind.
committed_t1() {
	(
		cd "$wtroot/t1" || exit 1
		mkdir -p app
		printf 't1\n' >app/t1.php
		git add app/t1.php
		git -c core.hooksPath=/dev/null commit -qm 'Add the t1 service'
	)
}

orphan
committed_t1
printf '%s ready\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >"$rundir/t1.status"
printf '{"is_error":false,"result":"ok"}\n' >"$rundir/t1.json"

run workflow run --plan-file "$T_TMP/plan.md"
is "$RC" 0 'the run picks up where the dead one stopped and completes'
like "$OUT" 'task t1: left dispatched by a run that is gone' \
	'it says the task was orphaned rather than pretending it dispatched it'
is "$(cat "$rundir/t1.state")" merged 'the orphaned task is collected, not restarted'
is "$(cat "$rundir/t1.dispatches")" 1 \
	'and no second worker was sent into a worktree that already had one'
is "$(cat "$rundir/t2.state")" merged 'the rest of the plan runs as usual'
is "$(git -C "$repo" rev-list --count "$base..integration/orphan-check")" 2 \
	'both commits are on the integration branch'
truthy "$([ ! -d "$wtroot/t1" ] && echo 0 || echo 1)" \
	'a run that ends with nothing dispatched does clean its worktrees up'

## ------------------------- collected with nothing to show: dispatched again

# The same orphan, but its worker said `started` and then died with its
# coordinator: nothing committed, nobody's word on the work. Collecting it
# fails it -- and failing it used to end the whole run in the same second,
# so getting the task done cost a `reap` and a second `workflow run` every
# time (friction #MT2WCVA2). The run that collected it dispatches it.
rm -rf "$rundir" "$wtroot"
git -C "$repo" worktree prune
for b in orphan-check/t1 orphan-check/t2 integration/orphan-check; do
	git -C "$repo" branch -D "$b" >/dev/null 2>&1
done
# The section above landed its work on the trunk and left the ref that
# records t1 as merged; here the task has never been done.
git -C "$repo" update-ref -d refs/workflow/orphan-check/t1 2>/dev/null || true

orphan
# A pid the backend can still see a record of and that is certainly not
# running: seen, not alive, and not paused either.
sh -c 'exit 0' &
dead=$!
wait "$dead" 2>/dev/null
printf '%s\n' "$dead" >"$rundir/t1.pid"
printf '%s started\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >"$rundir/t1.status"

run env WORKFLOW_DEADLINE_MIN=60 timeout 60 workflow run --plan-file "$T_TMP/plan.md"
is "$RC" 0 'the run finishes in one invocation'
like "$OUT" 'task t1: left dispatched by a run that is gone -- collecting it' \
	'the collected task is still collected'
like "$OUT" 'task t1: nobody read it and its retry is unspent -- pending again' \
	'and the run says it is taking it back rather than ending on it'
is "$(cat "$rundir/t1.dispatches")" 2 'it is dispatched again by the run that collected it'
is "$(cat "$rundir/t1.state")" merged 'and merges in that same invocation'
is "$(cat "$rundir/t2.state")" merged 'the rest of the plan runs as usual'

## ------------------------------ the machine went down: resumed at any attempt

# A power cut takes the session with it: a record of it may stay, but nothing
# lists it, and collecting it fails it as a worker that ended without a clean
# turn. On a task holding work that is an outage, not a verdict, so the run
# resumes it on this pass whatever its attempt count -- the retry guard above
# ended ebdify m1's join on its third power cut (friction #YRV5S6Z1).
rm -rf "$rundir" "$wtroot"
git -C "$repo" worktree prune
for b in orphan-check/t1 orphan-check/t2 integration/orphan-check; do
	git -C "$repo" branch -D "$b" >/dev/null 2>&1
done
# And the trunk back at the base: the work committed below is the task's own,
# not a second write over what the section above landed.
git -C "$repo" reset -q --hard "$base"
for t in t1 t2; do
	git -C "$repo" update-ref -d "refs/workflow/orphan-check/$t" 2>/dev/null || true
done

orphan
committed_t1
printf '3\n' >"$rundir/t1.dispatches"
sh -c 'exit 0' &
dead=$!
wait "$dead" 2>/dev/null
printf '%s\n' "$dead" >"$rundir/t1.pid"
printf '%s started\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >"$rundir/t1.status"

run env WORKFLOW_DEADLINE_MIN=60 timeout 60 workflow run --plan-file "$T_TMP/plan.md"
is "$RC" 0 'the run finishes in one invocation'
like "$OUT" 'task t1: its session is gone with work on it -- the machine went down; resuming it' \
	'and says the machine went down rather than failing the task'
is "$(cat "$rundir/t1.dispatches")" 4 'it is dispatched again past the retry guard'
is "$(cat "$rundir/t1.state")" merged 'and merges in that same invocation'

## --------------------------------------- the worker outlived its orchestrator

# Back to nothing: the run dir, the worktrees, the branches and the
# integration branch the half above left behind.
rm -rf "$rundir" "$wtroot"
git -C "$repo" worktree prune
for b in orphan-check/t1 orphan-check/t2 integration/orphan-check; do
	git -C "$repo" branch -D "$b" >/dev/null 2>&1
done

orphan
# This one is still going: a live pid in the pidfile is what the legacy
# template's liveness reads, and setsid gives it a process group of its own,
# so it outlives the orchestrator that started it exactly as a background
# session does -- and so stopping it cannot reach this test.
setsid sleep 30 &
live=$!
printf '%s\n' "$live" >"$rundir/t1.pid"
printf '%s started\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >"$rundir/t1.status"
# Committed, so the stall below fails it rather than trying it once more.
committed_t1

# The deadline is what ends it: nothing in that worktree moves, so the run
# adopts the worker, waits out the deadline and stops it.
run env WORKFLOW_DEADLINE_MIN=0.05 workflow run --plan-file "$T_TMP/plan.md"
like "$OUT" 'task t1: still working, from a run that is gone -- adopted' \
	'a worker still standing in its worktree is adopted, not replaced'
is "$(cat "$rundir/t1.dispatches")" 1 \
	'and it is not dispatched over the top of itself'
is "$(cat "$rundir/t1.state")" failed 'it is the deadline that ends an adopted worker'
truthy "$(kill -0 "$live" 2>/dev/null && echo 1 || echo 0)" \
	'and the run stopped the process it adopted, not one of its own'
# Already gone, if the check above held; this is only in case it did not.
kill "$live" 2>/dev/null || true

## ----------------------------------- the session that never existed at all

# The --session-id bug minted ids for sessions that never started. Adopting
# one used to sit out the whole stall deadline before redispatching (friction
# #9F7WT13K). At adoption, no record anywhere -- no process, no transcript,
# no status line, no result, no commit -- is known-dead: dispatch again now.
rm -rf "$rundir" "$wtroot"
git -C "$repo" worktree prune
for b in orphan-check/t1 orphan-check/t2 integration/orphan-check; do
	git -C "$repo" branch -D "$b" >/dev/null 2>&1
done

orphan
: >"$rundir/t1.status"
# A deadline the test cannot wait out: only an immediate redispatch finishes.
run env WORKFLOW_DEADLINE_MIN=60 timeout 60 workflow run --plan-file "$T_TMP/plan.md"
is "$RC" 0 'the run finishes without waiting out the deadline'
like "$OUT" 'never existed' 'it says the recorded session never existed'
is "$(cat "$rundir/t1.dispatches")" 2 'the ghost was dispatched again immediately'
is "$(cat "$rundir/t1.state")" merged 'and the second dispatch finished the task'

## ------------------------------------------ the session that vanished

# A worker that got far enough to report `started` before its session
# disappeared -- no pid, no process, no transcript -- is still gone, not
# still launching: the status line changes the note, not the outcome.
rm -rf "$rundir" "$wtroot"
git -C "$repo" worktree prune
for b in orphan-check/t1 orphan-check/t2 integration/orphan-check; do
	git -C "$repo" branch -D "$b" >/dev/null 2>&1
done

orphan
printf '%s started\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >"$rundir/t1.status"
run env WORKFLOW_DEADLINE_MIN=60 timeout 60 workflow run --plan-file "$T_TMP/plan.md"
is "$RC" 0 'the run finishes without waiting out the deadline'
like "$OUT" 'its session is gone and nothing was committed' \
	'it says the session is gone rather than never having existed'
is "$(cat "$rundir/t1.dispatches")" 2 'the vanished session was dispatched again immediately'
is "$(cat "$rundir/t1.state")" merged 'and the second dispatch finished the task'

## ------------------------------------- adoption starts a reading once, not twice

# t1 finished before its orchestrator died, same as the first case above, but
# this project also names a reader. Collecting it is what starts the reading
# -- and from that moment the reading is this run's own, not something a run
# that is gone left behind. The pass that rereads what a dead run left
# mid-flight must leave this one alone for the poll loop to judge.
rm -rf "$rundir" "$wtroot"
git -C "$repo" worktree prune
for b in orphan-check/t1 orphan-check/t2 integration/orphan-check; do
	git -C "$repo" branch -D "$b" >/dev/null 2>&1
done

orphan
committed_t1
printf '%s ready\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >"$rundir/t1.status"
printf '{"is_error":false,"result":"ok"}\n' >"$rundir/t1.json"

: >"$WF_TMP/reviews.log"
: >"$WF_TMP/hold-review"
env WORKFLOW_REVIEW_MODEL=fable WORKFLOW_DEADLINE_MIN=5 \
	workflow run --plan-file "$T_TMP/plan.md" >"$T_TMP/reading.log" 2>&1 &
runpid=$!

for _ in $(seq 1 100); do
	[ "$(cat "$rundir/t1.state" 2>/dev/null)" = reviewing ] && break
	sleep 0.2
done
is "$(cat "$rundir/t1.state" 2>/dev/null)" reviewing \
	'adoption starts the reading rather than merging straight through'

# The state file is written by the orchestrator the moment dispatch returns;
# the fake reader logs from a separately forked shell, which can still be
# behind it. Wait for the log rather than trusting the state file's timing.
for _ in $(seq 1 100); do
	[ "$(grep -c '^t1-review$' "$WF_TMP/reviews.log" 2>/dev/null)" -ge 1 ] && break
	sleep 0.2
done
is "$(grep -c '^t1-review$' "$WF_TMP/reviews.log")" 1 \
	'the reading it started is fresh, so it is not stopped and read again'
is "$(cat "$rundir/t1.review-tries")" 1 'one try recorded, not two'
rm -f "$WF_TMP/hold-review"

wait "$runpid"
is "$?" 0 'the run goes on to ship it'
is "$(cat "$rundir/t1.state")" merged 'the poll loop, not adoption, is what collects the ship verdict'
is "$(grep -c '^t1-review$' "$WF_TMP/reviews.log")" 1 'still one reading, start to finish'

## ------------------------ the machine went down under an open question

# The same outage on a worker that had stopped on its own question is not
# resumed: it waits on the answer, and a fresh attempt now would open
# without it.
rm -rf "$rundir" "$wtroot"
git -C "$repo" worktree prune
for b in orphan-check/t1 orphan-check/t2 integration/orphan-check; do
	git -C "$repo" branch -D "$b" >/dev/null 2>&1
done
git -C "$repo" reset -q --hard "$base"
for t in t1 t2; do
	git -C "$repo" update-ref -d "refs/workflow/orphan-check/$t" 2>/dev/null || true
done

orphan
committed_t1
printf '3\n' >"$rundir/t1.dispatches"
sh -c 'exit 0' &
dead=$!
wait "$dead" 2>/dev/null
printf '%s\n' "$dead" >"$rundir/t1.pid"
printf '%s blocked asked #AB12CD34\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >"$rundir/t1.status"

run env WORKFLOW_DEADLINE_MIN=60 WORKFLOW_QUESTION_MISSES=1 timeout 60 workflow run --plan-file "$T_TMP/plan.md"
unlike "$OUT" 'the machine went down' 'a task stopped on a question is not resumed as an outage'
is "$(cat "$rundir/t1.dispatches")" 3 'and is not dispatched again without its answer'

t_done
