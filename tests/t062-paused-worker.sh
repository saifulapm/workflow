#!/usr/bin/env bash
# A worker the usage limit pauses without ending: its pane stands and amx
# reads it idle, so it is listed but not alive, it says nothing past
# `started` in its own status file, and the pane names the limit that
# stopped it. Neither dead nor working, it is held to the stall deadline
# like a live worker, not collected the instant `alive` goes false (friction
# #17SPEY7R). A session that died with the machine looks the same in every
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
# to `ready`. With $WF_TMP/no-limit there the pane says nothing, which is a
# turn that simply ended. Status answers out of the phase, the evidence and
# the question last written for the name; stop is recorded.
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
		git -c core.hooksPath=/dev/null commit -qm "Add the $task file"
		printf '%s ready\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$status"
		printf 'done record\n' >"$AMX_DIR/$name.state"
		;;
	*)
		printf 'idle hooks\n' >"$AMX_DIR/$name.state"
		if [ -f "$WF_TMP/no-limit" ]; then
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
	printf '{"id":"%s","state":"%s","evidence":"%s","last_event":0,"session":"","question":%s,"last_words":%s}\n' \
		"$1" "$state" "$evidence" "$question" "$words"
	;;
stop)
	printf 'stop %s\n' "$1" >>"$WF_TMP/amx-stops"
	printf 'stopped record\n' >"$AMX_DIR/$1.state"
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

## ------------------------------------------------ reap_pass holds the line

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

# Twelve seconds, not three: the setup below -- waiting for the dispatch,
# then sitting out two polls -- has to fit inside the deadline with room to
# spare, and on a loaded machine a three-second budget was spent before the
# test got to what it measures (friction #CK3VG63H).
env WORKFLOW_MAX_WORKERS=2 WORKFLOW_DEADLINE_MIN=0.2 \
	workflow run --plan-file "$T_TMP/plan.md" >"$T_TMP/run.log" 2>&1 &
runpid=$!

for _ in $(seq 1 50); do
	[ "$(cat "$rundir/t1.state" 2>/dev/null)" = dispatched ] && break
	sleep 0.05
done
is "$(cat "$rundir/t1.state" 2>/dev/null)" dispatched 'the worker is dispatched'

# Two polls' worth of waiting, well short of the stall deadline: a paused
# worker used to be collected on the very next pass.
sleep 2.5
is "$(cat "$rundir/t1.state" 2>/dev/null)" dispatched \
	'paused -- listed, not alive, nothing past "started" -- is not collected before the deadline'

wait "$runpid"
is "$?" 1 'the run fails once the deadline is spent'
is "$(cat "$rundir/t1.state")" failed 'and the task is failed, past the deadline'
is "$(cat "$rundir/t1.dispatches")" 2 'after exactly one redispatch, same as a stalled worker'
like "$(cat "$rundir/t1.failed")" 'stalled with no sign of life' \
	'failed as a stall, never as a worker that reported and erred'
is "$(sort -u "$T_TMP/amx-stops" | grep -c '^stop wf-t1-')" 2 'each paused session is ended with amx stop, the same way a stalled one is'
# A pane amx is still releasing is a pane the next session must not be minted
# into (friction #RN9DB37H): a dispatch stops the attempt before it again,
# whatever ended it, and waits the kill grace out.
is "$(grep -c "^$(grep '^stop wf-t1-' "$T_TMP/amx-stops" | head -1)\$" "$T_TMP/amx-stops")" 2 \
	'and the first pane is stopped once more on the way into the redispatch'
is "$(cat "$rundir/t2.state")" merged 'the worker beside it merged as usual'

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
}

orphan paused-orphan pau1
printf 'idle hooks\n' >"$AMX_DIR/wf-t1-pau1.state"
printf 'Weekly limit reached - Retrying in 5h' >"$AMX_DIR/wf-t1-pau1.question"
run env WORKFLOW_DEADLINE_MIN=0.05 workflow run --plan-file "$T_TMP/paused-orphan.md"
like "$OUT" 'task t1: still working, from a run that is gone -- adopted' \
	'a paused task from a dead run is adopted like a live one, not collected at once'
is "$(cat "$orundir/t1.state")" failed 'the deadline still ends it eventually'
like "$(cat "$orundir/t1.failed")" 'stalled with no sign of life' \
	'failed as a stall, not mis-read as a worker that ran and erred'

## ------------------------------------- the session that died with the machine

# Same record, same status file, but amx's evidence says the pane is gone: a
# power cut. That used to read as paused and was waited on until the stall
# deadline; a record is not a listing, and the task is collected now.
orphan dead-orphan dea1
printf 'stopped gone\n' >"$AMX_DIR/wf-t1-dea1.state"
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
