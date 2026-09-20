#!/usr/bin/env bash
# A worker whose turn ends with nothing to judge -- no ready, no blocked, no
# question, no commit -- while its session still stands is nudged once in
# that session before the run does anything else: a model that narrated a
# tool call instead of making one, or a provider that cut its turn off, is
# one line away from carrying on, and a fresh dispatch re-reads everything
# it had (ebdify m1 gql died three times this way). The nudge is counted
# on its own, apart from `continued`; a second silent ending goes the way
# it always went, one more try, said as twice.
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"
export AMX_DIR="$T_TMP/amx"
mkdir -p "$AMX_DIR"
argv="$AMX_DIR/argv"
: >"$argv"

saw() {
	if grep -qF -- "$1" "$argv"; then ok "$2"; else notok "$2" "$(cat "$argv")"; fi
}
count() { grep -cF -- "$1" "$argv"; }
workers() { grep -cE "^new\|--name\|wf-$1-[0-9a-z]{4}\|" "$argv"; }

# The fake's first turn on `new` ends idle with nothing written. On `send`
# the worker of `once` does its task; the worker of `silent` stays silent
# again, and its second session (a fresh `new`) does the task.
write_exec "$T_TMP/fake-amx" <<'AMX'
#!/bin/sh
(IFS='|'; printf '%s\n' "$*") >>"$AMX_DIR/argv"
verb=$1
shift
say() { printf '%s %s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" "${2:-}" >>"$status"; }
commit() { git -c core.hooksPath=/dev/null commit -qm "$1" >/dev/null; }
work() {
	status=$(sed -n 's/^Append one line per state change to \(.*\):$/\1/p' "$brief")
	task=$(basename "$brief" .md)
	cd "$dir" || exit 1
	say started
	mkdir -p app
	printf 'final\n' >"app/$task.php"
	git add "app/$task.php"
	commit "Add the $task service"
	say ready
}
case $verb in
new)
	name= dir= text=
	while [ $# -gt 0 ]; do
		case $1 in
		--name) name=$2; shift 2 ;;
		--dir) dir=$2; shift 2 ;;
		--model | --effort | --parent | --role) shift 2 ;;
		--no-worktree | --bg | --json) shift ;;
		*) text=$1; shift ;;
		esac
	done
	brief=${text#Read }
	brief=${brief% and execute it exactly.}
	printf '%s\n' "$dir" >"$AMX_DIR/$name.dir"
	printf '%s\n' "$brief" >"$AMX_DIR/$name.brief"
	task=$(basename "$brief" .md)
	n=$(grep -cE "^new\|--name\|wf-$task-[0-9a-z]{4}\|" "$AMX_DIR/argv")
	if [ "$task" = silent ] && [ "$n" = 2 ]; then
		work
	fi
	printf 'idle hooks\n' >"$AMX_DIR/$name.state"
	;;
send)
	name=$1
	text=$2
	printf '%s\n' "$text" >"$AMX_DIR/$name.sent"
	dir=$(cat "$AMX_DIR/$name.dir")
	brief=$(cat "$AMX_DIR/$name.brief")
	task=$(basename "$brief" .md)
	[ "$task" = once ] && work
	printf 'idle hooks\n' >"$AMX_DIR/$name.state"
	;;
status)
	[ -f "$AMX_DIR/$1.state" ] || exit 1
	read -r state evidence <"$AMX_DIR/$1.state"
	printf '{"id":"%s","state":"%s","evidence":"%s","last_event":0,"session":""}\n' "$1" "$state" "$evidence"
	;;
stop) printf 'stopped record\n' >"$AMX_DIR/$1.state" ;;
esac
exit 0
AMX
export WORKFLOW_AMX="$T_TMP/fake-amx"

new_repo app
mem_register
"$MEM_BIN" project set verify true >/dev/null

cat >"$T_TMP/nudge.md" <<'PLAN'
# plan: nudge

- [ ] once Add the once service
      Files: app/once.php
      Verify: true
- [ ] silent Add the silent service
      Files: app/silent.php
      Verify: true
PLAN
export WORKFLOW_MAX_WORKERS=2 WORKFLOW_DEADLINE_MIN=0.5
run workflow run --plan-file "$T_TMP/nudge.md"
is "$RC" 0 'both tasks merge'
rundir="$XDG_STATE_HOME/workflow/runs/app/nudge"
line='Your last turn ended without a report and without a tool call. Continue; a provider cutoff looks the same from here.'

## ------------------------------------------------ one nudge, then the work

sess=$(cat "$rundir/once.session")
is "$(cat "$rundir/once.state")" merged 'once merged'
is "$(workers once)" 1 'in the one session it had'
saw "send|$sess|$line" 'after the ruling-1 line was sent into that session'
is "$(cat "$rundir/once.nudged")" 1 'the nudge is counted'
is "$(cat "$rundir/once.dispatches")" 1 'the attempt count did not move'
is "$(cat "$rundir/once.continued" 2>/dev/null)" '' 'and neither did the continuation count'
like "$OUT" "task once: its turn ended without a report -- nudged once in its session \\(session $sess\\)" 'the run says so'

## ------------------------------------- silent twice: one more try, said so

first=$(grep -E '^new\|--name\|wf-silent-[0-9a-z]{4}\|' "$argv" | head -1 | cut -d'|' -f3)
is "$(cat "$rundir/silent.state")" merged 'silent merged'
is "$(workers silent)" 2 'in a second session'
saw "send|$first|$line" 'after the first was nudged'
is "$(count "send|$first|")" 1 'exactly once'
is "$(cat "$rundir/silent.dispatches")" 2 'and the second silent ending spent the one more try'
like "$OUT" 'task silent: its worker ended twice without a report, once after a nudge -- one more try' 'said as twice'
saw "stop|$first" 'the silent session is stopped before the fresh one starts'

t_done
