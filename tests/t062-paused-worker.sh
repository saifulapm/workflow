#!/usr/bin/env bash
# A worker the usage limit pauses without ending: its pane stands and amx
# reads it idle, so it is listed but not alive, it says nothing past
# `started` in its own status file, and the pane names the limit that
# stopped it. It is waiting on the usage window, not stalled: no deadline
# stops it, and it merges once the window opens and it goes on (friction
# #17SPEY7R). So does a worker still running whose transcript ends in the
# harness's own rate-limit entry. A worker whose output only prints a limit
# phrase -- a test run, say -- is not limited: it is stalled at its deadline
# and stopped. A session that died with the machine looks the same in every
# way but one -- amx's evidence says the pane is gone -- and that one is
# collected at once (frictions #B3391C6H, #QT1PDNRK). So is a turn that
# ended with nothing written and no limit named: nothing is coming back for
# it (friction #VXFKQQ78).
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"
export AMX_DIR="$T_TMP/amx"
mkdir -p "$AMX_DIR"

# A dispatch writes `started` to the task's status file and goes idle at
# once -- paused, not working, with the pane still up and the provider's own
# line drawn on it -- unless the agent's name says it is one that should run
# to `ready`. With $WF_TMP/live there it stays working and the limit is the
# newest entry in its session's transcript instead, the one `<name>.session`
# names. With $WF_TMP/logs-only there it stays working and only its logs
# print a limit phrase. With $WF_TMP/no-limit there the pane says nothing,
# which is a turn that simply ended. Where to do the work when it wakes goes
# in `<name>.job`. Status answers out of the phase, the evidence, the session
# and the question last written for the name; logs out of `<name>.logs`;
# stop is recorded.
write_exec "$T_TMP/fake-amx" <<'AMX'
#!/bin/sh
verb=$1
shift
case $verb in
new | sub)
	name= dir= text=
	while [ $# -gt 0 ]; do
		case $1 in
		--name) name=$2; shift 2 ;;
		--dir) dir=$2; shift 2 ;;
		--model | --effort) shift 2 ;;
		--no-worktree | --bg | --json) shift ;;
		--parent | --role) shift 2 ;;
		*) text=$1; shift ;;
		esac
	done
	[ "$verb" = sub ] && printf '{"id":"%s","parent":null,"phase":"starting","answer":null,"evidence":"hooks"}\n' "$name"
	brief=${text#Read }
	brief=${brief% and execute it exactly.}
	status=$(sed -n 's/^Append one line per state change to \(.*\):$/\1/p' "$brief")
	case $name in
	wf-t2-* | wf-two-*)
		printf '%s started\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$status"
		cd "$dir" || exit 1
		task=$(basename "$brief" .md)
		file=$(sed -n 's/^ *Files: *//p' "$brief" | head -1)
		mkdir -p "$(dirname "$file")"
		printf '%s\n' "$task" >"$file"
		git add "$file"
		git -c core.hooksPath=/dev/null commit -qm "Add a file"
		printf '%s ready\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$status"
		printf 'done record\n' >"$AMX_DIR/$name.state"
		;;
	*)
		printf '%s\n%s\n%s\n' "$dir" "$status" "$(sed -n 's/^ *Files: *//p' "$brief" | head -1)" >"$AMX_DIR/$name.job"
		printf 'idle hooks\n' >"$AMX_DIR/$name.state"
		if [ -f "$WF_TMP/live" ]; then
			printf 'working hooks\n' >"$AMX_DIR/$name.state"
			printf '%s started\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$status"
			printf 'conv-%s\n' "$name" >"$AMX_DIR/$name.session"
			projects="$HOME/.claude/projects/$(printf %s "$dir" | sed 's/[^A-Za-z0-9]/-/g')"
			mkdir -p "$projects"
			cat >"$projects/conv-$name.jsonl" <<-'JSONL'
				{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Running the suite"}]}}
				{"type":"assistant","message":{"model":"<synthetic>","role":"assistant","content":[{"type":"text","text":"You've hit your weekly limit"}]},"isApiErrorMessage":true,"error":"rate_limit"}
				{"type":"system","subtype":"turn_duration"}
			JSONL
		elif [ -f "$WF_TMP/logs-only" ]; then
			printf 'working hooks\n' >"$AMX_DIR/$name.state"
			printf '%s started\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$status"
			printf 'Running the suite\nAPI Error: Rate limit reached for requests\n' >"$AMX_DIR/$name.logs"
		elif [ -f "$WF_TMP/no-limit" ]; then
			# A turn the provider cut off: nothing in the status file at
			# all, and the stop reason the only account of why.
			printf 'the turn ended: stopReason=length' >"$AMX_DIR/$name.words"
		else
			printf '%s started\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$status"
			printf 'Weekly limit reached - Retrying in 5h' >"$AMX_DIR/$name.question"
		fi
		;;
	esac
	;;
status)
	[ -f "$AMX_DIR/$1.state" ] || exit 1
	read -r state evidence <"$AMX_DIR/$1.state"
	question=null
	[ -f "$AMX_DIR/$1.question" ] && question=$(printf '{"text":"%s","options":[]}' "$(cat "$AMX_DIR/$1.question")")
	words=null
	[ -f "$AMX_DIR/$1.words" ] && words=$(printf '"%s"' "$(cat "$AMX_DIR/$1.words")")
	session=
	[ -f "$AMX_DIR/$1.session" ] && session=$(cat "$AMX_DIR/$1.session")
	printf '{"id":"%s","state":"%s","evidence":"%s","last_event":0,"session":"%s","question":%s,"last_words":%s}\n' \
		"$1" "$state" "$evidence" "$session" "$question" "$words"
	;;
logs)
	[ -f "$AMX_DIR/$1.logs" ] && cat "$AMX_DIR/$1.logs"
	;;
stop)
	printf 'stop %s\n' "$1" >>"$WF_TMP/amx-stops"
	printf 'stopped record\n' >"$AMX_DIR/$1.state"
	;;
send)
	# A turn that ended with nothing written is nudged once in its session
	# before it is judged (t085). What this file is about is the judgement,
	# so the pane refuses the line and the run judges the ending as it stands.
	exit 2
	;;
esac
exit 0
AMX
export WORKFLOW_AMX="$T_TMP/fake-amx"

new_repo app
mem_register
printf '{"name":"acme/app"}\n' >composer.json
printf '#!/bin/sh\nexit 0\n' >artisan
chmod +x artisan
write_exec bin/php <<-'PHP'
	#!/bin/sh
	exit 0
PHP
git add -A
git -c core.hooksPath=/dev/null commit -qm 'project files'
base=$(git rev-parse HEAD)
repo=$PWD

# The window opens: the worker named by `<rundir>/<task>.session` does its
# task where its job file says, reports ready, and its pane stops naming the
# limit.
wake() {
	name=$(cat "$1/t1.session")
	{
		read -r dir
		read -r status
		read -r file
	} <"$AMX_DIR/$name.job"
	(
		cd "$dir" || exit 1
		mkdir -p "$(dirname "$file")"
		printf 't1\n' >"$file"
		git add "$file"
		git -c core.hooksPath=/dev/null commit -qm 'Add a file'
	)
	printf '%s ready\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$status"
	rm -f "$AMX_DIR/$name.question" "$AMX_DIR/$name.logs" "$AMX_DIR/$name.session"
	printf 'idle hooks\n' >"$AMX_DIR/$name.state"
}

stops() { grep -c '^stop wf-t1-' "$T_TMP/amx-stops" 2>/dev/null; }

## ------------------------------------------- reap_pass waits on the window

cat >"$T_TMP/plan.md" <<'PLAN'
# plan: paused-worker

- [ ] t1 A worker that pauses without a final word
      Files: app/One.php
      Verify: true
- [ ] t2 A second task so the run is worth having
      Files: app/Two.php
      Verify: true
PLAN
rundir="$XDG_STATE_HOME/workflow/runs/app/paused-worker"

# Six seconds of deadline, slept past: the worker is never stalled, so the
# margin a loaded machine needs is on the far side of the deadline, not
# inside it (friction #CK3VG63H).
env WORKFLOW_MAX_WORKERS=2 WORKFLOW_DEADLINE_MIN=0.1 \
	workflow run --plan-file "$T_TMP/plan.md" >"$T_TMP/run.log" 2>&1 &
runpid=$!

for _ in $(seq 1 50); do
	[ "$(cat "$rundir/t1.state" 2>/dev/null)" = dispatched ] && break
	sleep 0.05
done
is "$(cat "$rundir/t1.state" 2>/dev/null)" dispatched 'the worker is dispatched'

sleep 9
is "$(cat "$rundir/t1.state" 2>/dev/null)" dispatched \
	'paused on the limit -- listed, not alive, nothing past "started" -- is not stalled past the deadline'
is "$(stops)" 0 'and its session is never stopped'
is "$(cat "$rundir/t2.state")" merged 'the worker beside it merged as usual'
[ -f "$rundir/usage-wait" ]
truthy "$?" 'with every worker on the limit, the run marks itself waiting'

wake "$rundir"
wait "$runpid"
is "$?" 0 'the run merges once the window opens'
is "$(cat "$rundir/t1.state")" merged 'and the waiting worker merged'
is "$(cat "$rundir/t1.dispatches")" 1 'in the one session it had'
is "$(grep -c 'waiting for the usage window' "$rundir/events")" 1 \
	'the wait is one event, however many polls it spanned'
is "$(grep -c 'waiting for the usage window' "$T_TMP/run.log")" 1 'and one line'

## ------------------------ a live worker the harness says the limit stopped

# Still running, as amx reads it, and silent past the deadline, with the
# harness's rate-limit entry the newest turn in its transcript: waiting, not
# stalled.
: >"$T_TMP/amx-stops"
: >"$WF_TMP/live"
cat >"$T_TMP/live.md" <<'PLAN'
# plan: live-limit

- [ ] t1 A worker that runs on with the limit in its logs
      Files: app/Three.php
      Verify: true
- [ ] t2 A second task so the run is worth having
      Files: app/Four.php
      Verify: true
PLAN
ldir="$XDG_STATE_HOME/workflow/runs/app/live-limit"
env WORKFLOW_MAX_WORKERS=2 WORKFLOW_DEADLINE_MIN=0.1 \
	workflow run --plan-file "$T_TMP/live.md" >"$T_TMP/live.log" 2>&1 &
runpid=$!
for _ in $(seq 1 50); do
	[ "$(cat "$ldir/t1.state" 2>/dev/null)" = dispatched ] && break
	sleep 0.05
done
sleep 9
is "$(cat "$ldir/t1.state" 2>/dev/null)" dispatched \
	'alive and silent past the deadline with a rate-limit entry in its transcript is not stalled'
is "$(stops)" 0 'and is never stopped'
wake "$ldir"
wait "$runpid"
is "$?" 0 'the run merges once it goes on'
is "$(cat "$ldir/t1.state")" merged 'and the worker merged'
rm -f "$WF_TMP/live"

## ------------------------------ a limit phrase in its logs and nowhere else

# The same live worker, but the limit line is only something its output
# printed and the harness wrote no entry for it: a test that greps for the
# phrase looks exactly like this. It is stalled at its deadline and stopped,
# dispatched once more since it committed nothing, and failed the second time.
: >"$T_TMP/amx-stops"
: >"$WF_TMP/logs-only"
cat >"$T_TMP/logs-only.md" <<'PLAN'
# plan: logs-only

- [ ] t1 A worker whose output prints a limit phrase
      Files: app/Five.php
      Verify: true
- [ ] t2 A second task so the run is worth having
      Files: app/Six.php
      Verify: true
PLAN
odir="$XDG_STATE_HOME/workflow/runs/app/logs-only"
run env WORKFLOW_MAX_WORKERS=2 WORKFLOW_DEADLINE_MIN=0.05 timeout 120 \
	workflow run --plan-file "$T_TMP/logs-only.md"
is "$RC" 1 'the run ends with the task failed rather than waiting'
is "$(cat "$odir/t1.state")" failed 'the task is failed'
like "$(cat "$odir/t1.failed")" 'stalled' 'as a stall'
is "$(printf '%s\n' "$OUT" | grep -c 'task t1: nothing has moved for')" 2 \
	'reaped at the deadline of each dispatch'
[ "$(stops)" -ge 2 ]
truthy "$?" 'its session stopped each time'
is "$(cat "$odir/t1.dispatches")" 2 'after one more dispatch'
is "$(cat "$odir/t2.state")" merged 'the worker beside it merged as usual'
rm -f "$WF_TMP/logs-only"

## --------------------------------------- adopt_stale keeps it as adopted

# The pane still stands, idle -- the shape the usage limit leaves
# (#17SPEY7R) -- and only "started" said: paused, not dead, from a run that
# no longer exists to watch it.
orphan() {
	cat >"$T_TMP/$1.md" <<-PLAN
	# plan: $1

	- [ ] t1 A worker left by a run that is gone
	      Files: app/One.php
	      Verify: true
	- [ ] two A second task so the run is worth having
	      Files: app/Two.php
	      Verify: true
	PLAN
	orundir="$XDG_STATE_HOME/workflow/runs/app/$1"
	owtroot="$XDG_STATE_HOME/workflow/worktrees/app/$1"
	mkdir -p "$orundir"
	printf '%s\n' "$base" >"$orundir/base_sha"
	printf 'dispatched\n' >"$orundir/t1.state"
	printf '1\n' >"$orundir/t1.dispatches"
	printf '%s\n' "$(date -u +%s)" >"$orundir/t1.dispatched_at"
	git -C "$repo" worktree add -q -b "$1/t1" "$owtroot/t1" "$base"
	git -C "$repo" worktree add -q -b "$1/two" "$owtroot/two" "$base"
	printf '%s\n' "wf-t1-$2" >"$orundir/t1.session"
	printf '%s started\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >"$orundir/t1.status"
	printf '%s\n%s\n%s\n' "$owtroot/t1" "$orundir/t1.status" app/One.php >"$AMX_DIR/wf-t1-$2.job"
}

orphan paused-orphan pau1
printf 'idle hooks\n' >"$AMX_DIR/wf-t1-pau1.state"
printf 'Weekly limit reached - Retrying in 5h' >"$AMX_DIR/wf-t1-pau1.question"
env WORKFLOW_DEADLINE_MIN=0.05 workflow run --plan-file "$T_TMP/paused-orphan.md" >"$T_TMP/orphan.log" 2>&1 &
runpid=$!
sleep 6
like "$(cat "$T_TMP/orphan.log")" 'task t1: still working, from a run that is gone -- adopted' \
	'a paused task from a dead run is adopted like a live one, not collected at once'
is "$(cat "$orundir/t1.state")" dispatched 'and the deadline does not end it'
wake "$orundir"
wait "$runpid"
is "$?" 0 'the run merges once the window opens'
is "$(cat "$orundir/t1.state")" merged 'and the adopted worker merged'

## ------------------------------------- the session that died with the machine

# Same record, same status file, but amx's evidence says the pane is gone: a
# power cut. That used to read as paused and was waited on until the stall
# deadline; a record is not a listing, and the task is collected now.
orphan dead-orphan dea1
printf 'stopped gone\n' >"$AMX_DIR/wf-t1-dea1.state"
# Its redispatch ends its turn with no limit named, so the run ends on it
# rather than waiting on a window.
: >"$WF_TMP/no-limit"
run env WORKFLOW_DEADLINE_MIN=0.05 timeout 120 workflow run --plan-file "$T_TMP/dead-orphan.md"
is "$RC" 1 'the run ends on its own rather than sitting out a deadline on the dead session'
unlike "$OUT" 'still working, from a run that is gone -- adopted' \
	'a session whose pane is gone is not adopted as paused'
like "$OUT" 'task t1: left dispatched by a run that is gone -- collecting it' \
	'it is collected at once, like a worker that ended'
like "$OUT" 'task t1: failed -- the worker ended without a clean turn' \
	'and failed for what it did: reported started, then ended unclean'
# Nobody read that work and the retry was unspent, so the run that collected
# it sends it out again rather than ending on it (friction #MT2WCVA2).
is "$(cat "$orundir/t1.dispatches")" 2 'and dispatched again in the same invocation'

## ------------------------------- a turn that ended, with no limit named

# The same pane, up and idle, with nothing on it saying a limit paused it and
# nothing written to the status file: a provider that cut the turn off
# mid-reasoning leaves exactly this. Nothing is coming back for it, so the
# run collects it on the next poll and reports the stop reason rather than
# holding the task to a stall deadline it spends in full while `wait` never
# fires (friction #VXFKQQ78).
: >"$WF_TMP/no-limit"
cat >"$T_TMP/ended.md" <<'PLAN'
# plan: ended

- [ ] t1 A worker whose turn ended with nothing written
      Files: app/One.php
      Verify: true
- [ ] t2 A second task so the run is worth having
      Files: app/Two.php
      Verify: true
PLAN
edir="$XDG_STATE_HOME/workflow/runs/app/ended"
run env WORKFLOW_MAX_WORKERS=2 WORKFLOW_DEADLINE_MIN=0.2 timeout 120 \
	workflow run --plan-file "$T_TMP/ended.md"
is "$RC" 1 'the run ends with the task failed'
is "$(cat "$edir/t1.state")" failed 'the task is failed'
unlike "$(cat "$edir/t1.failed")" 'stalled' \
	'not as a stall: the turn ended, it did not go quiet'
like "$(cat "$edir/t1.failed")" 'the worker stopped without reporting ready -- it last said: the turn ended: stopReason=length' \
	'and the stop reason the pane gave is the reason the run records'
like "$(cat "$edir/events")" 'failed t1' 'the failure is an event, so wait fires on it'
is "$(cat "$edir/t2.state")" merged 'the worker beside it merged as usual'
rm -f "$WF_TMP/no-limit"

t_done
