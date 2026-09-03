#!/usr/bin/env bash
# The reader at the merge gate (plan gate-reviewer). A task whose Verify is
# green on integration is read once more by a model in a clean context; a
# `fix` verdict takes the path a red Verify takes, and the findings wait in
# the run dir for the redispatched worker, whose brief names the file.
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"

# The fake worker. t1 writes a draft on its first attempt and, dispatched
# again with the file already on its branch, commits the fix the reviewer
# asked for. hold stays alive until released so the run stays live and
# redispatch has something to reach. Everything else commits once.
write_exec "$T_TMP/worker.sh" <<'FAKE'
#!/bin/sh
task=$1; status=$3; brief=$5
say() { printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" >>"$status"; }
commit() { git -c core.hooksPath=/dev/null commit -qm "$1"; }
say started
cp "$brief" "$WF_TMP/brief-$task-$(date +%s%N)"
case $task in
hold)
	while [ ! -f "$WF_TMP/release-hold" ]; do
		say progress
		sleep 0.5
	done
	mkdir -p app
	printf 'hold\n' >app/hold.php
	git add app/hold.php
	commit 'Add the hold service'
	;;
t1)
	mkdir -p app
	if [ -f app/t1.php ]; then
		printf 'fixed\n' >>app/t1.php
		git add app/t1.php
		commit 'Fix what the review found'
	else
		printf 'draft\n' >app/t1.php
		git add app/t1.php
		commit 'Add the t1 service'
	fi
	;;
*)
	mkdir -p app
	printf '%s\n' "$task" >"app/$task.php"
	git add "app/$task.php"
	commit "Add the $task service"
	;;
esac
say ready
printf '{"is_error":false,"result":"ok"}\n'
FAKE

# The fake reviewer, standing in for `claude -p`. It logs what it was handed,
# wants fixes for a t1 diff that lacks the fix, prints no verdict at all for
# t2, and ships everything else.
write_exec "$T_TMP/reviewer.sh" <<'FAKE'
#!/bin/sh
model=$1; wt=$2; prompt=$3; out=$4
task=$(sed -n '1s/^# Review of task \([a-z0-9]*\) .*/\1/p' "$prompt")
printf '%s %s %s\n' "$model" "$task" "$wt" >>"$WF_TMP/reviews.log"
case $task in
t2)
	printf 'I read it twice and could not decide.\n' >"$out"
	;;
t1)
	if grep -q '^+fixed$' "$prompt"; then
		printf 'VERDICT: ship\nThe diff is clean.\n' >"$out"
	else
		printf 'Reading...\n\n**VERDICT: fix**\n1. app/t1.php:1 -- says draft; the Done line wants the fix.\n' >"$out"
	fi
	;;
*)
	printf 'VERDICT: ship\n' >"$out"
	;;
esac
FAKE

export FAKE="$T_TMP/worker.sh"
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$FAKE" {task} {worktree} {status} {session} {brief}'"'"' > {out} 2> {err} &'
export REVIEWER="$T_TMP/reviewer.sh"
export WORKFLOW_REVIEW_CMD='sh "$REVIEWER" {model} {worktree} {prompt} {out} {err}'

new_repo app
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

# Two tasks at least: a one-task plan is refused as not worth a worker.
plan() {
	cat >"$T_TMP/$1.md" <<-EOF
	# plan: $1

	## Spec

	Ruling 1. The t1 service says what the reviewer asked for.

	- [ ] t1 Add the t1 service
	      Files: app/t1.php
	      Verify: true
	      Done: app/t1.php carries the fix
	- [ ] side Add the side service
	      Files: app/side.php
	      Verify: true
	$2
	EOF
}

## ------------------------------------------- nobody named: nobody reads

plan quiet ''
run env WORKFLOW_DEADLINE_MIN=0.5 workflow run --plan-file "$T_TMP/quiet.md"
is "$RC" 0 'with no review-model and no variable the run merges as before'
is "$(cat "$XDG_STATE_HOME/workflow/runs/app/quiet/t1.state")" merged 'the draft merges unread'
[ -f "$WF_TMP/reviews.log" ] && notok 'and the reviewer was never called' "$(cat "$WF_TMP/reviews.log")" || ok 'and the reviewer was never called'
unlike "$OUT" 'reading the diff' 'the run never said anyone was reading'

## ------------------------------------------------ the project names one

"$MEM_BIN" project set review-model fable >/dev/null
plan live '- [ ] hold Stay alive until released
      Files: app/hold.php
      Verify: true
- [ ] t2 Add the t2 service  [after: t1]
      Files: app/t2.php
      Verify: true'
rundir="$XDG_STATE_HOME/workflow/runs/app/live"

env WORKFLOW_MAX_WORKERS=2 WORKFLOW_DEADLINE_MIN=0.5 \
	workflow run --plan-file "$T_TMP/live.md" >"$T_TMP/run.log" 2>&1 &
runpid=$!

for _ in $(seq 1 100); do
	[ "$(cat "$rundir/t1.state" 2>/dev/null)" = failed ] && break
	sleep 0.2
done
is "$(cat "$rundir/t1.state" 2>/dev/null)" failed 'a fix verdict fails the task'
like "$(cat "$rundir/t1.failed")" '^the reviewer wants fixes first \(review 1\) -- read ' 'with a note that says so and names the file'
review=$(sed 's/.* -- read //' "$rundir/t1.failed")
is "$review" "$rundir/t1.review" 'which is <task>.review in the run dir'
like "$(cat "$review")" 'app/t1.php:1 -- says draft' 'and it holds the findings'
is "$(cat "$rundir/t1.reviews")" 1 'the fix count is one'
unlike "$(git log --format=%s integration/live)" 'Add the t1 service' 'integration was reset: the draft is not on it'
is "$(cat "$rundir/t1.merging" 2>/dev/null)" '' 'and no merge is recorded as in flight'
like "$(cat "$rundir/t1.review-prompt")" '# Review of task t1 before it merges' 'the prompt names the task'
like "$(cat "$rundir/t1.review-prompt")" 'Ruling 1\. The t1 service' 'carries the plan of record'
like "$(cat "$rundir/t1.review-prompt")" 'Done: app/t1.php carries the fix' 'the task block'
like "$(cat "$rundir/t1.review-prompt")" '^\+draft$' 'and the diff'
like "$(cat "$WF_TMP/reviews.log")" "^fable t1 $XDG_STATE_HOME/workflow/worktrees/app/live/_integration\$" 'the reviewer ran as the named model in the integration worktree'

run workflow redispatch t1
is "$RC" 0 'redispatch reaches the live run'
for _ in $(seq 1 150); do
	[ "$(cat "$rundir/t1.state" 2>/dev/null)" = merged ] && break
	sleep 0.2
done
is "$(cat "$rundir/t1.state" 2>/dev/null)" merged 'the second attempt ships and merges'
is "$(cat "$rundir/t1.dispatches")" 2 'as a second attempt'
is "$(cat "$rundir/t1.reviews")" 1 'the fix count stays at one'
like "$(cat "$review")" 'VERDICT: ship' 'the review file now holds the ship verdict'
second=$(ls -t "$WF_TMP"/brief-t1-* | head -1)
like "$(cat "$second")" 'This is attempt 2\. The last one ended: the reviewer wants fixes first \(review 1\) -- read ' 'the redispatched brief says why and where'
like "$(cat "$second")" "$rundir/t1\.review" 'naming the review file'

# t2 follows the first wave, and its reviewer never decides.
: >"$WF_TMP/release-hold"
wait "$runpid"
is "$?" 1 'the run stops short over t2'
is "$(cat "$rundir/hold.state")" merged 'hold merged once released'
is "$(cat "$rundir/side.state")" merged 'so did side'
is "$(cat "$rundir/t2.state" 2>/dev/null)" failed 'a reviewer that prints no verdict fails the task'
like "$(cat "$rundir/t2.failed")" '^the review returned no verdict -- read ' 'and says so, naming the file'
is "$(grep -c '^fable t2 ' "$WF_TMP/reviews.log")" 2 'after one more reading'
[ -f "$rundir/t2.reviews" ] && notok 'a missing verdict is not a fix' "$(cat "$rundir/t2.reviews")" || ok 'a missing verdict is not a fix'
like "$(cat "$T_TMP/run.log")" 'task t1: fable is reading the diff' 'the log says who read'
like "$(cat "$T_TMP/run.log")" 'task t1: the reviewer says ship' 'and what it said'
like "$(cat "$T_TMP/run.log")" 'Failed - the review returned no verdict -- read .*: t2' 'the report groups t2 under the missing verdict'
is "$(grep -c '^fable hold ' "$WF_TMP/reviews.log")" 1 'every merge is read once'

## --------------------------------------------- the variable beats the key

plan override ''
: >"$WF_TMP/reviews.log"
run env WORKFLOW_DEADLINE_MIN=0.5 WORKFLOW_REVIEW_MODEL=opus workflow run --plan-file "$T_TMP/override.md"
is "$RC" 1 'the draft fails review under the run-level model too'
like "$(cat "$WF_TMP/reviews.log")" '^opus t1 ' 'and it was that model that read'
like "$(cat "$WF_TMP/reviews.log")" '^opus side ' 'every task of the run'

plan off ''
: >"$WF_TMP/reviews.log"
run env WORKFLOW_DEADLINE_MIN=0.5 WORKFLOW_REVIEW_MODEL= workflow run --plan-file "$T_TMP/off.md"
is "$RC" 0 'an empty variable turns the reading off for the run'
is "$(cat "$XDG_STATE_HOME/workflow/runs/app/off/t1.state")" merged 'and the draft merges unread'
is "$(wc -c <"$WF_TMP/reviews.log")" 0 'nobody was called'
