#!/usr/bin/env bash
# workflow run: the orchestration fixture of AC7. No real workers -- every
# shape a worker can take is a fake dispatched through WORKFLOW_WORKER_CMD, and
# the deadline is injected in seconds.
source "$(dirname -- "$0")/lib.sh"
t_init

CONC="$T_TMP/conc"
mkdir -p "$CONC"
export CONC
export WF_TMP="$T_TMP"

# The fake worker. It stands in for `claude -p`: it is handed the task id, its
# worktree, its status file and its session id, it works, it reports, and it
# prints a result JSON on stdout exactly where the real thing would.
write_exec "$T_TMP/fake-worker.sh" <<'FAKE'
#!/bin/sh
task=$1; wt=$2; status=$3; session=$4
live="$CONC/$task.live"
: >"$live"
trap 'rm -f "$live"; exit 143' TERM INT
printf '%s %s\n' "$task" "$session" >>"$WF_TMP/sessions.log"

say() { printf '%s %s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" "${2:-}" >>"$status"; }
commit() { git -c core.hooksPath=/dev/null commit -qm "$1"; }
done_json() { printf '{"is_error":false,"result":"ok"}\n'; }

say started
case "$task" in
t1)
	mkdir -p app/Services
	printf '<?php // pricing\n' >app/Services/CartPricing.php
	git add app/Services/CartPricing.php
	commit 'Add the cart pricing service'
	say ready 'merge-ready'
	done_json
	;;
t2)
	# Its parent must already be on the integration branch by now.
	if git -C "$WF_MAIN" merge-base --is-ancestor \
		"$(git -C "$WF_MAIN" rev-parse fixture-run/t1)" \
		"$(git -C "$WF_MAIN" rev-parse integration/fixture-run)" 2>/dev/null; then
		printf 'yes\n' >"$WF_TMP/t2-saw-parent-merged"
	else
		printf 'no\n' >"$WF_TMP/t2-saw-parent-merged"
	fi
	# Its parent's work should be in this worktree already: the run caught the
	# worktree up to the integration branch before waking it (#VC7PAESB).
	[ -f app/Services/CartPricing.php ] &&
		printf 'yes\n' >"$WF_TMP/t2-had-parents-work"
	# A worker that brings the branch in for itself is the other shape, and the
	# gate must not charge it with its parent's file for doing so (#A2JXGNB8).
	git merge -q --ff-only integration/fixture-run
	mkdir -p app/Services
	printf '<?php // totals\n' >app/Services/CartTotals.php
	git add app/Services/CartTotals.php
	commit 'Add the cart totals service'
	say ready 'merge-ready'
	done_json
	;;
red)
	# Clean exit, green here, red once it sits on top of t1.
	printf 'x\n' >RED
	git add RED
	commit 'Add the thing the suite hates'
	say ready 'merge-ready'
	done_json
	;;
own1)
	# A tracked file it does not own, plus a rename out of its patterns.
	mkdir -p app/Services
	printf '<?php\n' >app/Services/Owned1.php
	printf 'extra\n' >>'docs/with space.md'
	git mv app/Services/Legacy.php 'app/Services/Renamed Legacy.php'
	git add -A
	commit 'Reach outside the task'
	say ready 'merge-ready'
	done_json
	;;
own2)
	mkdir -p app/Services notes
	printf '<?php\n' >app/Services/Owned2.php
	git add app/Services/Owned2.php
	commit 'Add the owned file'
	printf 'scratch\n' >notes/scratch.txt
	say ready 'merge-ready'
	done_json
	;;
hang)
	sh -c 'sleep 300' &
	printf '%s\n' "$!" >>"$WF_TMP/hang-children"
	say progress 'thinking'
	sleep 300
	;;
touch)
	# Nothing in the worktree moves; only the transcript says it is alive.
	slug=$(printf '%s' "$wt" | sed 's/[^A-Za-z0-9]/-/g')
	tp="$HOME/.claude/projects/$slug/$session.jsonl"
	mkdir -p "$(dirname "$tp")"
	i=0
	while [ "$i" -lt 9 ]; do
		printf '{"type":"assistant"}\n' >>"$tp"
		sleep 0.5
		i=$((i + 1))
	done
	mkdir -p app/Services
	printf '<?php\n' >app/Services/Touch.php
	git add app/Services/Touch.php
	commit 'Add the touch service'
	say ready 'merge-ready'
	done_json
	;;
stuck)
	# Reports why it cannot go on, then exits cleanly: the failure must say it.
	say blocked 'waiting on the sibling task'
	done_json
	;;
colons)
	# Punctuates the state it was asked to write bare. The work is
	# merge-ready and the run must read it as such (friction #W2SY30WH).
	mkdir -p app/Services
	printf '<?php\n' >app/Services/Colons.php
	git add app/Services/Colons.php
	commit 'Add the colons service'
	say 'ready::' 'merge-ready'
	done_json
	;;
noword)
	# A state the protocol never taught it: failed, and the run has to name
	# the vocabulary rather than quote a word nothing documents.
	say finished 'all done here'
	done_json
	;;
esac
rm -f "$live"
FAKE

export FAKE="$T_TMP/fake-worker.sh"
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$FAKE" {task} {worktree} {status} {session}'"'"' > {out} 2> {err} &'

## ------------------------------------------------------------- the project

new_repo app
export WF_MAIN="$PWD"
mem_register
mkdir -p app/Services docs
printf '{"name":"acme/app"}\n' >composer.json
printf '#!/bin/sh\nexit 0\n' >artisan
chmod +x artisan
write_exec bin/php <<-'EOF'
	#!/bin/sh
	# Green, unless the pricing service and RED are both here: that pair only
	# exists once the red task has been rebased onto the integration branch.
	if [ -f RED ] && [ -f app/Services/CartPricing.php ]; then exit 1; fi
	exit 0
EOF
printf '<?php\n' >app/Services/Legacy.php
printf 'docs\n' >'docs/with space.md'
git add -A
git -c core.hooksPath=/dev/null commit -qm 'project files'
base=$(git rev-parse HEAD)

"$MEM_BIN" plan --stdin >/dev/null <<'EOF'
# plan: fixture-run

- [ ] t1 Add the pricing service
      Files: app/Services/CartPricing.php
      Verify: true
- [ ] own1 Reach outside the task
      Files: app/Services/Owned1.php
      Verify: true
- [ ] own2 Leave a stray file behind
      Files: app/Services/Owned2.php
      Verify: true
- [ ] hang Never finish
      Files: app/Services/Hang.php
      Verify: true
- [ ] touch Stay alive in the transcript only
      Files: app/Services/Touch.php
      Verify: true
- [ ] t2 Add the totals service [after: t1]
      Files: app/Services/CartTotals.php
      Verify: true
- [ ] red Commit what the suite hates [after: t1]
      Files: RED
      Verify: true
- [ ] stuck Report blocked and stop
      Files: app/Services/Stuck.php
      Verify: true
- [ ] colons Punctuate the ready report
      Files: app/Services/Colons.php
      Verify: true
- [ ] noword Report a state the protocol has no word for
      Files: app/Services/Noword.php
      Verify: true
EOF

## ------------------------------------------------------------------ the run

rundir="$XDG_STATE_HOME/workflow/runs/app/fixture-run"

# A sampler counts the worker processes the run believes it has going.
(
	while [ ! -f "$T_TMP/run-done" ]; do
		n=0
		for f in "$rundir"/*.pid; do
			[ -f "$f" ] || continue
			kill -0 "$(cat "$f")" 2>/dev/null && n=$((n + 1))
		done
		printf '%s\n' "$n" >>"$T_TMP/conc.samples"
		sleep 0.1
	done
) &
sampler=$!

export WORKFLOW_MAX_WORKERS=2 WORKFLOW_DEADLINE_MIN=0.05
run workflow run
run_rc=$RC
run_out=$OUT
touch "$T_TMP/run-done"
wait "$sampler" 2>/dev/null

is "$run_rc" 1 'the run reports failed work with exit 1'

## ------------------------------------------------------------- assertions

is "$(cat "$rundir/t1.state")" merged 't1: the succeeding fake merged'
is "$(cat "$rundir/t2.state")" merged \
	't2: a dependent that took the integration branch in still merged'
is "$(cat "$T_TMP/t2-saw-parent-merged" 2>/dev/null)" yes \
	't2 was dispatched only after t1 was on the integration branch'
is "$(cat "$T_TMP/t2-had-parents-work" 2>/dev/null)" yes \
	"and t2's worktree already held its parent's work when it woke"

max=$(sort -n "$T_TMP/conc.samples" | tail -1)
is "$max" 2 'concurrency: two workers at once, never three'

is "$(cat "$rundir/own1.state")" failed 'the tracked-file ownership violator is refused'
like "$run_out" 'with space' 'and the whole record for the unowned path with a space is reported'
like "$run_out" 'Renamed Legacy' 'and the rename that left its patterns is reported'
is "$(cat "$rundir/own2.state")" failed 'the untracked-file ownership violator is refused'
like "$run_out" 'notes/scratch.txt' 'and the stray untracked file is named'

is "$(cat "$rundir/red.state")" failed 'the clean exit with red tests is failed'
is "$(cat "$rundir/red.dispatches")" 1 'and it is not redispatched'
like "$(cat "$rundir/red.failed")" 'red' 'and the reason is the suite, not the worker'

is "$(cat "$rundir/stuck.state")" failed 'the worker that reported blocked is failed'
like "$(cat "$rundir/stuck.failed")" 'blocked: waiting on the sibling task' \
	"and the failure quotes the worker's own report"

is "$(cat "$rundir/colons.state")" merged \
	'a ready report the worker punctuated is still a ready report'
is "$(cat "$rundir/noword.state")" failed 'a state the protocol has no word for fails'
like "$(cat "$rundir/noword.failed")" 'not one of started, progress, ready, blocked' \
	'and the failure names the vocabulary instead of quoting a word alone'
like "$(cat "$rundir/t1.verify")" 'true' 'dispatch recorded the task Verify command in the run dir'

is "$(cat "$rundir/hang.state")" failed 'the hung worker ends up failed'
is "$(cat "$rundir/hang.dispatches")" 2 'after exactly one redispatch'

# A redispatched worker used to wake up to the same fixed text as the first
# attempt, with the status file truncated behind it (friction #YCW7ND6Z).
hangbrief=$(cat "$XDG_CACHE_HOME/workflow/briefs/app/fixture-run/hang.md")
like "$hangbrief" 'This is attempt 2' 'the redispatch brief says which attempt this is'
like "$hangbrief" 'stalled with nothing committed' 'and what became of the one before it'
like "$hangbrief" "last report was 'progress" "and what that attempt last reported"
s1=$(grep '^hang ' "$T_TMP/sessions.log" | head -1 | cut -d' ' -f2)
s2=$(grep '^hang ' "$T_TMP/sessions.log" | tail -1 | cut -d' ' -f2)
isnt "$s1" "$s2" 'the redispatch minted a new session uuid'
like "$s2" '^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$' \
	'and it is a uuid, which is what --session-id insists on'
alive=0
while IFS= read -r p; do
	[ -n "$p" ] && kill -0 "$p" 2>/dev/null && alive=$((alive + 1))
done <"$T_TMP/hang-children"
is "$alive" 0 'the whole process group went, children included'

is "$(cat "$rundir/touch.state")" merged 'the transcript-only worker was never killed'

## ---------------------------------------------------- the integration branch

ahead=$(git rev-list --count "$base..integration/fixture-run")
is "$ahead" 4 'integration is ahead of base_sha by the four merged tasks'
run git merge-base --is-ancestor "$(git rev-parse fixture-run/red 2>/dev/null || echo HEAD)" integration/fixture-run
isnt "$RC" 0 'the red task never landed on integration'

git worktree add -q "$T_TMP/check" integration/fixture-run
cd "$T_TMP/check" || exit 1
run workflow verify
is "$RC" 0 'integration is green at the end of the run'
cd "$WF_MAIN" || exit 1
git worktree remove --force "$T_TMP/check"

is "$(git worktree list | grep -c .)" 1 'git worktree list is clean again'
is "$(git branch --list 'fixture-run/t1' | grep -c .)" 0 'merged task branches are deleted'
is "$(git branch --list 'fixture-run/own1' | grep -c .)" 1 'failed task branches are kept'
like "$("$MEM_BIN" plan)" '\[x\] t1' 'mem plan --tick checked the merged task off'

## ------------------------------------------------- plans run refuses to run

new_repo tiny
mem_register
cat >"$T_TMP/one-task.md" <<'PLAN'
# plan: one-task

- [ ] t1 Just do this one thing
      Files: src/**
      Verify: true
PLAN
run workflow run --plan-file "$T_TMP/one-task.md"
is "$RC" 0 'a single-task plan is not worth orchestrating'
like "$OUT" 'one task' 'and run says so instead of spinning up a worker'
is "$(git worktree list | grep -c .)" 1 'and it made no worktrees'

cat >"$T_TMP/broken.md" <<'PLAN'
# plan: broken

- [ ] a First [after: nope]
      Files: x
      Verify: true
- [ ] b Second
      Files: y
      Verify: true
PLAN
run workflow run --plan-file "$T_TMP/broken.md"
is "$RC" 2 'a plan error is exit 2, not a failed run'
like "$OUT" 'nope' 'and names what is wrong with the plan'

run workflow run --plan-file "$T_TMP/does-not-exist.md"
is "$RC" 2 'an unreadable plan file is exit 2'

## ------------------------------------------------- a monorepo child project

# mem resolves a monorepo subdir to its child project, so a run typed there
# must read the child's plan slot and record itself under the child's name --
# not the root's, which is where the toplevel cwd would land every mem call
# (frictions #GCYJFZT3, #FFSFMBDH).

new_repo mono
mem_register
printf '{"name":"acme/mono"}\n' >composer.json
printf '#!/bin/sh\nexit 0\n' >artisan
chmod +x artisan
write_exec bin/php <<-'EOF'
	#!/bin/sh
	exit 0
EOF
mkdir -p apps/child/src
printf 'seed\n' >apps/child/src/seed.txt
git add -A
git -c core.hooksPath=/dev/null commit -qm 'mono files'
"$MEM_BIN" project add apps/child >/dev/null

cd apps/child
"$MEM_BIN" plan --stdin >/dev/null <<'EOF'
# plan: child-solo

- [ ] t1 Just one thing
      Files: apps/child/src/**
      Verify: true
EOF
run workflow run
is "$RC" 0 'a run typed in the child subdir finds the child plan'
like "$OUT" 'one task' 'and refuses to orchestrate it, which proves it was read'

write_exec "$T_TMP/child-worker.sh" <<'CFAKE'
#!/bin/sh
task=$1; status=$3
printf '%s started\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$status"
printf 'x\n' >"apps/child/src/$task.txt"
git add "apps/child/src/$task.txt"
git -c core.hooksPath=/dev/null commit -qm "Add $task"
printf '%s ready merge-ready\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$status"
printf '{"is_error":false,"result":"ok"}\n'
CFAKE
export CFAKE="$T_TMP/child-worker.sh"
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$CFAKE" {task} {worktree} {status} {session}'"'"' > {out} 2> {err} &'

cat >"$T_TMP/child-run.md" <<'PLAN'
# plan: child-run

- [ ] a1 Add one file
      Files: apps/child/src/a1.txt
      Verify: true
- [ ] a2 Add another file
      Files: apps/child/src/a2.txt
      Verify: true
PLAN
run workflow run --plan-file "$T_TMP/child-run.md"
is "$RC" 0 'the child run ends green'
[ -d "$XDG_STATE_HOME/workflow/runs/child/child-run" ] && recorded=yes || recorded=no
is "$recorded" yes 'the run is recorded under the child project name'

run workflow status --json
like "$OUT" '"project": *"child"' 'status typed in the same subdir resolves the child'
like "$OUT" '"plan": *"child-run"' 'and lists the run the dispatch recorded'
