#!/usr/bin/env bash
# The reader at the merge gate (plan gate-reviewer). A task whose Verify is
# green on integration is read once more by a model in a clean context; a
# `fix` verdict takes the path a red Verify takes, and the findings wait in
# the run dir for the redispatched worker, whose brief names the file. The
# reader is dispatched like a worker, through the same template, so one fake
# plays both parts: a task id ending in -review is a reading.
source "$(dirname -- "$0")/lib.sh"
t_init

# lib.sh runs every other test unread; naming a reader is what this one is
# about, so the run reads the project key again.
unset WORKFLOW_REVIEW_MODEL

export WF_TMP="$T_TMP"

# The fake worker. t1 writes a draft on its first attempt and, dispatched
# again with the file already on its branch, commits the fix the reviewer
# asked for. hold stays alive until released so the run stays live and
# redispatch has something to reach. Everything else commits once.
#
# As the reader (task <id>-review) it logs what it was handed, wants fixes
# for a t1 diff that lacks the fix (holding that verdict back while
# hold-review exists, so the run can be watched going on around a reading),
# writes no verdict at all for t2, asks a question and edits the tree for t3,
# dies on a provider's own line for wall, and ships everything else.
write_exec "$T_TMP/worker.sh" <<'FAKE'
#!/bin/sh
task=$1; wt=$2; status=$3; session=$4; brief=$5; model=$6
say() { printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" >>"$status"; }
commit() { git -c core.hooksPath=/dev/null commit -qm "$1"; }
case $task in
*-review)
	answer=$(sed -n 's/^    Answer file: //p' "$brief")
	cp "$brief" "$WF_TMP/review-brief-${task%-review}-$(date +%s%N)"
	printf '%s %s %s\n' "$model" "${task%-review}" "$wt" >>"$WF_TMP/reviews.log"
	case $task in
	t2-review) printf 'I read it twice and could not decide.\n' >"$answer" ;;
	wall-review)
		slug=$(printf '%s' "$wt" | sed -E 's/[^A-Za-z0-9]/-/g')
		dir="$HOME/.claude/projects/$slug"
		mkdir -p "$dir"
		printf '%s\n' "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"You've reached your Fable limit for this session.\"}]}}" >"$dir/$session.jsonl"
		printf 'stopped mid-turn\n' >"$answer"
		;;
	ceiling-review)
		# No transcript at all -- a custom template's shape -- and no
		# verdict either; the only record of why is what lands on
		# stderr through the dispatch's own redirect.
		echo 'Error: rate limit exceeded, please retry.' >&2
		printf 'nothing useful\n' >"$answer"
		;;
	floor-review)
		# A session that ran a while before hitting the wall: ordinary
		# text in the transcript, and the limit only on stderr.
		slug=$(printf '%s' "$wt" | sed -E 's/[^A-Za-z0-9]/-/g')
		dir="$HOME/.claude/projects/$slug"
		mkdir -p "$dir"
		printf '%s\n' "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"Reading floor.php against ruling 1...\"}]}}" >"$dir/$session.jsonl"
		echo 'Error: rate limit exceeded, please retry.' >&2
		printf 'nothing useful\n' >"$answer"
		;;
	curfew-review)
		# Refused at launch: no transcript ever starts, no verdict
		# comes, and the process itself outlives the review deadline,
		# so the run's own stop() is what ends it. Its stderr is the
		# only record of why.
		echo 'Error: this session is not logged in.' >&2
		sleep 30
		;;
	t3-review)
		mem ask 'may I run the suite myself?' >/dev/null 2>&1
		printf 'meddling\n' >"$wt/app/t3.php"
		printf 'VERDICT: ship\n' >"$answer"
		;;
	t1-review)
		if grep -q '^+fixed$' "$brief"; then
			printf 'VERDICT: ship\nThe diff is clean.\n' >"$answer"
		else
			while [ -f "$WF_TMP/hold-review" ]; do sleep 0.2; done
			printf 'Reading...\n\n**VERDICT: fix**\n1. app/t1.php:1 -- says draft; the Done line wants the fix.\n' >"$answer"
		fi
		;;
	twice-review)
		printf 'VERDICT: fix\n1. app/twice.php:1 -- never good enough.\n' >"$answer"
		;;
	redgate-review)
		# The reader starts before the suite does, so it is still reading
		# when the gate turns red; it never gets to write a verdict, since
		# the run stops it first.
		while [ -f "$WF_TMP/hold-redgate-review" ]; do sleep 0.2; done
		printf 'VERDICT: ship\n' >"$answer"
		;;
	*) printf 'VERDICT: ship\n' >"$answer" ;;
	esac
	exit 0
	;;
esac
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
twice)
	mkdir -p app
	if [ -f app/twice.php ]; then
		printf 'again\n' >>app/twice.php
		git add app/twice.php
		commit 'Never good enough, apparently'
	else
		printf 'twice\n' >app/twice.php
		git add app/twice.php
		commit 'Add the twice service'
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

export FAKE="$T_TMP/worker.sh"
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$FAKE" {task} {worktree} {status} {session} {brief} {model}'"'"' > {out} 2> {err} &'

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

## ---------------------------------------- nobody named: the run is refused

# The gate is the reason the workflow merges anything unattended, so a project
# that has named nobody is asked to decide before a worker is started, not
# after the sessions are spent.
plan quiet ''
run env WORKFLOW_DEADLINE_MIN=0.5 workflow run --plan-file "$T_TMP/quiet.md"
is "$RC" 2 'with no review-model and no variable the run is refused'
like "$OUT" 'nobody is named to read what this run merges' 'saying what is missing'
like "$OUT" 'mem project set review-model <model>' 'naming the remedy'
like "$OUT" 'mem project set review-model none' 'the recorded way to mean nobody'
like "$OUT" 'WORKFLOW_REVIEW_MODEL=' 'and the way to run one unread on purpose'
[ -f "$WF_TMP/reviews.log" ] && notok 'and the reviewer was never called' "$(cat "$WF_TMP/reviews.log")" || ok 'and the reviewer was never called'
[ -d "$XDG_STATE_HOME/workflow/worktrees/app/quiet" ] && notok 'and no worker was dispatched' 'the run set up worktrees' || ok 'and no worker was dispatched'
is "$(git worktree list | grep -c .)" 1 'nor any worktree in the checkout'

## ------------------------------------------------ the project names one

"$MEM_BIN" project set review-model fable >/dev/null
plan live '- [ ] hold Stay alive until released
      Files: app/hold.php
      Verify: true
- [ ] t2 Add the t2 service  [after: t1]
      Files: app/t2.php
      Verify: true
- [ ] t3 Add the t3 service  [after: t1]
      Files: app/t3.php
      Verify: true
- [ ] twice Add the twice service
      Files: app/twice.php
      Verify: true'
rundir="$XDG_STATE_HOME/workflow/runs/app/live"
wtroot="$XDG_STATE_HOME/workflow/worktrees/app/live"

: >"$WF_TMP/hold-review"
env WORKFLOW_MAX_WORKERS=2 WORKFLOW_DEADLINE_MIN=0.5 \
	workflow run --plan-file "$T_TMP/live.md" >"$T_TMP/run.log" 2>&1 &
runpid=$!

# The reading does not hold the run: while the reader has t1's diff, the
# slot t1's worker gave up is filled and the next task starts -- on the
# integration commit before t1's fast-forward, which nothing has recorded.
for _ in $(seq 1 100); do
	[ "$(cat "$rundir/t1.state" 2>/dev/null)" = reviewing ] && break
	sleep 0.2
done
is "$(cat "$rundir/t1.state" 2>/dev/null)" reviewing 'a task whose diff is being read is reviewing'
like "$(cat "$rundir/t1.merging" 2>/dev/null)" '^[0-9a-f]+ [0-9a-f]+$' 'with its fast-forward on record as in flight'
for _ in $(seq 1 100); do
	[ "$(cat "$rundir/hold.state" 2>/dev/null)" = dispatched ] && break
	sleep 0.2
done
is "$(cat "$rundir/hold.state" 2>/dev/null)" dispatched 'and the run dispatches the next task while the reader reads'
unlike "$(git -C "$wtroot/hold" log --format=%s)" 'Add the t1 service' 'onto integration as it stood before the unrecorded fast-forward'
rm -f "$WF_TMP/hold-review"

for _ in $(seq 1 100); do
	grep -q 'task t1: dispatched again with the findings on the next free slot' "$T_TMP/run.log" 2>/dev/null && break
	sleep 0.2
done
# The redispatch that follows a first fix verdict is instant, on the same
# pass that judges it: write_field, unwind, the copy to review.1, fail_task
# (its own git and mem spawns) and only then the marker and this line, so
# the poll waits on the last thing that pass writes, not the first -- a
# reviews==1 poll can land inside that window and read t1.failed or
# t1.review.1 before either exists.
is "$(cat "$rundir/t1.reviews")" 1 'the fix count is one'
like "$(cat "$rundir/t1.failed")" '^the reviewer wants fixes first \(review 1\) -- read ' 'with a note that says so and names the file'
review=$(sed 's/.* -- read //' "$rundir/t1.failed")
is "$review" "$rundir/t1.review.1" 'which is <task>.review.<n> in the run dir'
like "$(cat "$review")" 'app/t1.php:1 -- says draft' 'and it holds the findings'
like "$(cat "$rundir/t1.review-prompt")" '# Review of task t1 before it merges' 'the prompt names the task'
like "$(cat "$rundir/t1.review-prompt")" 'Ruling 1\. The t1 service' 'carries the plan of record'
like "$(cat "$rundir/t1.review-prompt")" 'Done: app/t1.php carries the fix' 'the task block'
like "$(cat "$rundir/t1.review-prompt")" '^\+draft$' 'and the diff'
like "$(cat "$rundir/t1.review-prompt")" 'artisan test' 'and the gate commands the run detected'
like "$(cat "$WF_TMP/reviews.log")" "^fable t1 $XDG_STATE_HOME/workflow/worktrees/app/live/_integration\$" 'the reader ran as the named model in the integration worktree'
like "$(cat "$rundir/t1.review-session")" '.' 'and its session is recorded, so it can be watched'
like "$(cat "$T_TMP/run.log")" 'task t1: fable is reading the diff' 'the log says who is reading'
like "$(cat "$T_TMP/run.log")" 'task t1: dispatched again with the findings on the next free slot' 'nothing had to call workflow redispatch'

# Nobody calls `workflow redispatch t1`: the first fix verdict sent it back
# by itself, and the second attempt ships on the free slot that frees up.
for _ in $(seq 1 150); do
	[ "$(cat "$rundir/t1.state" 2>/dev/null)" = merged ] && break
	sleep 0.2
done
is "$(cat "$rundir/t1.state" 2>/dev/null)" merged 'the second attempt ships and merges'
is "$(cat "$rundir/t1.dispatches")" 2 'as a second attempt'
is "$(cat "$rundir/t1.reviews")" 1 'the fix count stays at one'
like "$(cat "$rundir/t1.review")" 'VERDICT: ship' 't1.review now holds the ship verdict'
like "$(cat "$review")" 'app/t1.php:1 -- says draft' 'and t1.review.1 still keeps the first fix verdict'
second=$(ls -t "$WF_TMP"/brief-t1-* | head -1)
like "$(cat "$second")" 'This is attempt 2\. The last one ended: the reviewer wants fixes first \(review 1\) -- read ' 'the redispatched brief says why and where'
like "$(cat "$second")" "$rundir/t1\.review\.1" 'naming the kept review file'

# The second reading carries the first, so it verdicts what it was already
# told rather than reading the diff cold a second time (ruling 3).
firstprompt=$(ls -t "$WF_TMP"/review-brief-t1-* | tail -1)
secondprompt=$(ls -t "$WF_TMP"/review-brief-t1-* | head -1)
unlike "$(cat "$firstprompt")" '## Earlier readings of this task' 't1'"'"'s first prompt carries no earlier readings'
like "$(cat "$secondprompt")" '### Reading 1' 't1'"'"'s second prompt carries the first reading'
like "$(cat "$secondprompt")" 'says draft' 'verbatim, findings and all'
like "$(cat "$secondprompt")" 'First verdict each earlier finding: addressed or not' 'and the verdict-then-reread instruction'

# t2 follows the first wave, and its reviewer never decides.
: >"$WF_TMP/release-hold"
wait "$runpid"
is "$?" 1 'the run stops short over t2'
is "$(cat "$rundir/hold.state")" merged 'hold merged once released'
is "$(cat "$rundir/side.state")" merged 'so did side'
is "$(cat "$rundir/t2.state" 2>/dev/null)" failed 'a reviewer that prints no verdict fails the task'
like "$(cat "$rundir/t2.failed")" '^the review returned no verdict -- read ' 'and says so, naming the file'
like "$(cat "$rundir/t2.failed")" "$rundir/t2\\.review" 'naming the reader'"'"'s answer file'
unlike "$(cat "$rundir/t2.failed")" 'review-err' 'not review-err, since this reading left nothing there'
like "$(cat "$rundir/t2.failed")" '\(session .+\)$' 'and naming the session the last reading ran as'
is "$(grep -c '^fable t2 ' "$WF_TMP/reviews.log")" 2 'after one more reading'
[ -f "$rundir/t2.reviews" ] && notok 'a missing verdict is not a fix' "$(cat "$rundir/t2.reviews")" || ok 'a missing verdict is not a fix'
is "$(cat "$rundir/t3.state" 2>/dev/null)" failed 'a reader that touched the tree fails the task'
like "$(cat "$rundir/t3.failed")" '^the reviewer changed the tree, which voids the reading -- read ' 'and says so'

# twice's reader never ships: the first fix sends it back by itself, same as
# t1, but a second fix in a row is the orchestrator's call, not the run's.
is "$(cat "$rundir/twice.state" 2>/dev/null)" failed 'a reader that never ships fails after the second round'
like "$(cat "$rundir/twice.failed")" '\(review 2\)' 'with the second fix verdict recorded'
is "$(cat "$rundir/twice.reviews")" 2 'two rounds counted'
is "$(cat "$rundir/twice.dispatches")" 2 'dispatched exactly twice, not a third time'
[ -f "$rundir/twice.redispatch" ] && notok 'and no marker waits behind it' "$(cat "$rundir/twice.redispatch")" || ok 'and no marker waits behind it'
like "$(cat "$rundir/twice.review.1")" 'never good enough' 'the first fix is kept'
like "$(cat "$rundir/twice.review.2")" 'never good enough' 'and so is the second'

is "$(git -C "$XDG_STATE_HOME/workflow/worktrees/app/live/_integration" status --porcelain 2>/dev/null | wc -l)" 0 'and the integration worktree was put back'
unlike "$(git log --format=%s integration/live)" 'Add the t3 service' 'with t3 not on integration'
like "$(cat "$T_TMP/run.log")" 'task t1: the reviewer says ship' 'the log says what the reader said'
like "$(cat "$T_TMP/run.log")" 'Failed - the review returned no verdict -- read .*: t2' 'the report groups t2 under the missing verdict'
is "$(grep -c '^fable hold ' "$WF_TMP/reviews.log")" 1 'every merge is read once'
run_out "$MEM_BIN" questions --for orchestrator --json
like "$OUT" '"task": ?"live/t3-review"' 'a question the reader asked anyway is tagged with the reading'
like "$OUT" 'moot: the reading of t3 ended without waiting on it' 'and was closed as moot when the reading ended'

## --------------------------------------------- the variable beats the key

plan override ''
: >"$WF_TMP/reviews.log"
run env WORKFLOW_DEADLINE_MIN=0.5 WORKFLOW_MODEL=sonnet WORKFLOW_REVIEW_MODEL=opus \
	workflow run --plan-file "$T_TMP/override.md"
is "$RC" 0 'the draft still ships, after its own fix round, under the run-level model too'
is "$(grep -c '^opus t1 ' "$WF_TMP/reviews.log")" 2 'the fix and the ship both read under it'
like "$(cat "$WF_TMP/reviews.log")" '^opus side ' 'every task of the run'

## ------------------------------------- the reader is the one who wrote it

# A model reading its own work agrees with itself, so naming the workers' own
# model is the same as naming nobody -- and a run nobody reads is refused
# before a worker is started, never merged quietly unread.
plan mirror ''
: >"$WF_TMP/reviews.log"
run env WORKFLOW_DEADLINE_MIN=0.5 WORKFLOW_MODEL=sonnet WORKFLOW_REVIEW_MODEL=sonnet \
	workflow run --plan-file "$T_TMP/mirror.md"
is "$RC" 2 'a reader that names the workers own model is refused'
like "$OUT" 'the workers write with sonnet, and sonnet is the same model' 'the run says why'
like "$OUT" 'name another reader with `mem project set review-model <model>`' 'and what to do about it'
is "$(wc -c <"$WF_TMP/reviews.log")" 0 'nobody was called'
[ -e "$XDG_STATE_HOME/workflow/runs/app/mirror/t1.state" ] && notok 'and no worker was dispatched' 'a task has a state' || ok 'and no worker was dispatched'

# One model under two spellings. The alias the CLI takes and the full id
# start the same model, so a reader named by one for workers running under
# the other is still a model reading its own work.
plan alias ''
: >"$WF_TMP/reviews.log"
run env WORKFLOW_DEADLINE_MIN=0.5 WORKFLOW_MODEL=claude-opus-5 WORKFLOW_REVIEW_MODEL=opus \
	workflow run --plan-file "$T_TMP/alias.md"
is "$RC" 2 'a reader named by alias for the model the workers run on by full id is refused too'
is "$(wc -c <"$WF_TMP/reviews.log")" 0 'nobody was called'
like "$OUT" 'the workers write with claude-opus-5, and opus is the same model' 'and the run names both spellings'

# The workers' default is a model like any other. A project that never set
# `model` still runs its workers on opus, so naming opus as the reader there
# is naming the workers' own model -- the shape a project falls into by
# setting review-model alone. Kept before the `model` key is ever written,
# because an empty value is a usage error and not a way to clear it back.
plan default ''
: >"$WF_TMP/reviews.log"
run env WORKFLOW_DEADLINE_MIN=0.5 WORKFLOW_REVIEW_MODEL=opus workflow run --plan-file "$T_TMP/default.md"
is "$RC" 2 'a reader that names the default the workers fell back to is refused'
is "$(wc -c <"$WF_TMP/reviews.log")" 0 'nobody was called'
like "$OUT" 'the workers write with opus, and opus is the same model' 'and the run says so by name'

# The project key and the run's model meet the same way.
"$MEM_BIN" project set model fable >/dev/null
"$MEM_BIN" project set review-model fable >/dev/null
plan keys ''
: >"$WF_TMP/reviews.log"
run env WORKFLOW_DEADLINE_MIN=0.5 workflow run --plan-file "$T_TMP/keys.md"
is "$RC" 2 'the two project keys naming one model are refused as well'
is "$(wc -c <"$WF_TMP/reviews.log")" 0 'with nobody called'

# And a cheaper worker under a frontier reader still gets read.
"$MEM_BIN" project set model sonnet >/dev/null
plan cheap ''
: >"$WF_TMP/reviews.log"
run env WORKFLOW_DEADLINE_MIN=0.5 workflow run --plan-file "$T_TMP/cheap.md"
is "$RC" 0 'a worker model the reader does not share is read as before'
like "$(cat "$WF_TMP/reviews.log")" '^fable t1 ' 'by the named reader'

plan off ''
: >"$WF_TMP/reviews.log"
run env WORKFLOW_DEADLINE_MIN=0.5 WORKFLOW_REVIEW_MODEL= workflow run --plan-file "$T_TMP/off.md"
is "$RC" 0 'an empty variable turns the reading off for the run'
is "$(cat "$XDG_STATE_HOME/workflow/runs/app/off/t1.state")" merged 'and the draft merges unread'
is "$(wc -c <"$WF_TMP/reviews.log")" 0 'nobody was called'

## ------------------------------------------------- a reader that hit a limit

# A reading that ends on a provider's own refusal is not a defect a second
# reading is going to resolve: the task fails on the first one and says which
# line stopped it.
cat >"$T_TMP/limit.md" <<-'EOF'
# plan: limit

## Spec

Ruling 1. Nothing to say.

- [ ] wall Add the wall service
      Files: app/wall.php
      Verify: true
- [ ] floor Add the floor service
      Files: app/floor.php
      Verify: true
- [ ] ceiling Add the ceiling service
      Files: app/ceiling.php
      Verify: true
- [ ] curfew Add the curfew service
      Files: app/curfew.php
      Verify: true
EOF
rundir="$XDG_STATE_HOME/workflow/runs/app/limit"
: >"$WF_TMP/reviews.log"
run env WORKFLOW_DEADLINE_MIN=0.5 WORKFLOW_REVIEW_DEADLINE_MIN=0.15 \
	workflow run --plan-file "$T_TMP/limit.md"
is "$RC" 1 'a reader that hit a provider limit fails the run'
is "$(cat "$rundir/wall.state" 2>/dev/null)" failed 'the task is failed'
like "$(cat "$rundir/wall.failed")" "^the reader hit a provider limit: You've reached your Fable limit for this session\\. \\(session " 'the note names the line and the session'
like "$(cat "$rundir/wall.review-err")" "You've reached your Fable limit for this session\\." 'review-err carries the same line'
is "$(grep -c '^fable wall ' "$WF_TMP/reviews.log")" 1 'only one reading was tried'

# A reading with no transcript at all -- a custom template's shape -- still
# has its own stderr to read: the dispatch put it in review-err directly, and
# an empty transcript must not blank that out before the limit line is read.
is "$(cat "$rundir/ceiling.state" 2>/dev/null)" failed 'a reader with no transcript still fails on its own stderr'
like "$(cat "$rundir/ceiling.failed")" "^the reader hit a provider limit: Error: rate limit exceeded, please retry\\. \\(session " 'the note names the line and the session'
like "$(cat "$rundir/ceiling.review-err")" 'rate limit exceeded' 'review-err keeps the stderr the dispatch captured'
is "$(grep -c '^fable ceiling ' "$WF_TMP/reviews.log")" 1 'only one reading was tried'

# A reading with ordinary transcript text but the limit only on stderr -- the
# shape a real session takes when it ran a while before hitting the wall.
# The stderr the dispatch captured must survive next to the last words, not
# be overwritten by them, or the limit line is lost and a second reading
# meets the same wall.
is "$(cat "$rundir/floor.state" 2>/dev/null)" failed 'a reader with transcript text and a limit on stderr fails too'
like "$(cat "$rundir/floor.failed")" "^the reader hit a provider limit: Error: rate limit exceeded, please retry\\. \\(session " 'the note names the line and the session'
like "$(cat "$rundir/floor.review-err")" 'Reading floor\.php against ruling 1\.\.\.' 'review-err keeps the transcript text'
like "$(cat "$rundir/floor.review-err")" 'rate limit exceeded' 'and the stderr line next to it'
is "$(grep -c '^fable floor ' "$WF_TMP/reviews.log")" 1 'only one reading was tried'

# A reader that never returns a verdict and is still going when the review's
# own deadline stops it -- launch refused, no transcript, the wall on stderr
# alone. The deadline branch has to read review-err the same way the ended
# branch does, or the diagnosis sits unread while a second reading burns the
# full deadline again.
is "$(cat "$rundir/curfew.state" 2>/dev/null)" failed 'a reader stopped at the deadline fails on its own stderr too'
like "$(cat "$rundir/curfew.failed")" "^the reader hit a provider limit: Error: this session is not logged in\\. \\(session " 'the note names the line and the session'
like "$(cat "$rundir/curfew.review-err")" 'not logged in' 'review-err keeps the stderr the dispatch captured'
is "$(grep -c '^fable curfew ' "$WF_TMP/reviews.log")" 1 'only one reading was tried'

## ------------------------------------ the recorded way to run one unread

# An absent key is a project that never decided, and the run stops to ask;
# `review-model none` is the decision, and it belongs in the store beside the
# other project keys rather than in the environment of every run. The run goes
# ahead unread and says whose choice that was.
"$MEM_BIN" project set review-model none >/dev/null
plan nobody ''
: >"$WF_TMP/reviews.log"
run env WORKFLOW_DEADLINE_MIN=0.5 workflow run --plan-file "$T_TMP/nobody.md"
is "$RC" 0 'review-model none runs unread with no variable set'
like "$OUT" 'nobody reads this run: review-model is none' 'and the run says so'
is "$(cat "$XDG_STATE_HOME/workflow/runs/app/nobody/t1.state")" merged 'the draft merges unread'
is "$(wc -c <"$WF_TMP/reviews.log")" 0 'nobody was called'
is "$(cat "$XDG_STATE_HOME/workflow/runs/app/nobody/review-model")" '' 'and the run records no reader'

# The variable still beats the key, in both directions: a project that records
# nobody can still have one run read.
plan reader ''
: >"$WF_TMP/reviews.log"
run env WORKFLOW_DEADLINE_MIN=0.5 WORKFLOW_REVIEW_MODEL=fable \
	workflow run --plan-file "$T_TMP/reader.md"
is "$RC" 0 'the variable names a reader over a recorded none'
unlike "$OUT" 'nobody reads this run' 'so the run does not say nobody reads it'
like "$(cat "$WF_TMP/reviews.log")" '^fable t1 ' 'and that model read the diff'

## --------------------------------- a refusal says what the last run recorded

# An orchestrator launched against a project whose key was never set cannot
# answer "who reads?" from the plan (friction #7GVER0M5); the refusal says what
# the project's latest run recorded, and the decision can be made from that.
"$MEM_BIN" project unset review-model >/dev/null
plan fresh ''
run env WORKFLOW_DEADLINE_MIN=0.5 workflow run --plan-file "$T_TMP/fresh.md"
is "$RC" 2 'with the key unset a new plan is refused again'
like "$OUT" 'the last run here, reader, read with fable\.' 'and the refusal names what the latest run recorded'

# A run picked up again after a stop reads with what it recorded when it
# began, key or no key: `nobody` recorded no reader, and goes on unread. Its
# work is landed first, or preflight stops it over the integration branch.
git merge -q integration/nobody
run env WORKFLOW_DEADLINE_MIN=0.5 workflow run --plan-file "$T_TMP/nobody.md"
is "$RC" 0 'a run that recorded no reader when it began is not refused for the key'
like "$OUT" 'nobody reads this run: it recorded no reader when it began' 'and says whose choice that was'

## ------------------------------------- a red gate stops a reader still reading

# The reader starts before the gate's own suite runs, not after: it works
# alongside the gate rather than waiting on it. A red gate voids the reading
# outright -- the run stops it mid-turn and fails the merge on the suite's
# own words, never on a verdict that never came.
new_repo redgate
mem_register
"$MEM_BIN" project set review-model fable >/dev/null
write_exec "$T_TMP/redgate-verify.sh" <<'FAKE'
#!/bin/sh
printf 'not ok 1 - the suite is red\n'
exit 1
FAKE
"$MEM_BIN" project set verify "$T_TMP/redgate-verify.sh" >/dev/null

"$MEM_BIN" plan --stdin >/dev/null <<'EOF'
# plan: redgate

- [ ] redgate The one whose gate goes red while its reader is still reading
      Files: app/redgate.php
      Verify: true
- [ ] side A second task, so the plan is worth a worker
      Files: app/side.php
      Verify: true
EOF

rundir="$XDG_STATE_HOME/workflow/runs/redgate/redgate"
: >"$WF_TMP/hold-redgate-review"
run env WORKFLOW_DEADLINE_MIN=0.5 workflow run
is "$RC" 1 'the run stops short over the red gate'
is "$(cat "$rundir/redgate.state")" failed 'the task is failed'
like "$(cat "$rundir/redgate.failed")" '^the suite is red once the change sits on integration' "the gate's own note is the reason"
like "$(cat "$rundir/redgate.failed")" 'not ok 1 - the suite is red' 'naming the failing check'
[ -f "$rundir/redgate.review" ] && notok 'no verdict was judged' "$(cat "$rundir/redgate.review")" || ok 'no verdict was judged'
readerpid=$(cat "$rundir/redgate.review-pid" 2>/dev/null)
kill -0 "$readerpid" 2>/dev/null
isnt "$?" 0 'and its reader process was stopped'

## ------------------------- the task is reviewing while the suite still runs

# A reader is live for the whole gate suite, not just once it answers: the
# suite is the long part, and a run killed while it runs must find the
# reading on record, not a task still marked dispatched beside an orphan
# reader. The state flips to reviewing before the suite starts, not after it
# ends.
new_repo slowgate
mem_register
"$MEM_BIN" project set review-model fable >/dev/null
# Marks the instant its own subprocess starts, then holds until released --
# no clock race: `gate` writes the task's state before it spawns this, so by
# the time the marker exists the state must already have flipped.
write_exec "$T_TMP/slowgate-verify.sh" <<'FAKE'
#!/bin/sh
: >"$WF_TMP/slowgate-verify-started"
while [ -f "$WF_TMP/hold-slowgate-verify" ]; do sleep 0.1; done
printf 'ok\n'
exit 0
FAKE
"$MEM_BIN" project set verify "$T_TMP/slowgate-verify.sh" >/dev/null

"$MEM_BIN" plan --stdin >/dev/null <<'EOF'
# plan: slowgate

- [ ] slowgate The one whose gate suite takes a moment while its reader reads
      Files: app/slowgate.php
      Verify: true
- [ ] side A second task, so the plan is worth a worker
      Files: app/side.php
      Verify: true
EOF

rundir="$XDG_STATE_HOME/workflow/runs/slowgate/slowgate"
: >"$WF_TMP/hold-slowgate-verify"
env WORKFLOW_DEADLINE_MIN=0.5 workflow run >"$T_TMP/slowgate.log" 2>&1 &
runpid=$!
for _ in $(seq 1 200); do
	[ -f "$WF_TMP/slowgate-verify-started" ] && break
	sleep 0.1
done
[ -f "$WF_TMP/slowgate-verify-started" ]
truthy "$?" 'the gate suite started'
is "$(cat "$rundir/slowgate.state" 2>/dev/null)" reviewing 'and the task is already reviewing, not still dispatched, once it is running'
rm -f "$WF_TMP/hold-slowgate-verify"
wait "$runpid"
is "$?" 0 'the run goes on to merge it once the suite and the reading both clear'
is "$(cat "$rundir/slowgate.state")" merged 'the task merged'
