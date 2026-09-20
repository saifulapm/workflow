#!/usr/bin/env bash
# The status protocol after m1-lessons ruling 2. `workflow report <state>
# "<note>"` writes the `<utc> <state> <note>` line itself, from the task
# worktree, so no worker composes the time or orders the fields; a line a
# worker still wrote by hand with the state first is read as what it meant;
# and the status file is never truncated -- a second attempt or a
# continuation appends a marker, the gate reads only what came after it, and
# what the last attempt said stays readable where it was said.
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"

write_exec "$T_TMP/fake-worker.sh" <<'FAKE'
#!/bin/sh
task=$1; wt=$2; status=$3; session=$4; brief=$5
say() { printf '%s %s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" "${2:-}" >>"$status"; }
commit() { git -c core.hooksPath=/dev/null commit -qm "$1"; }
done_json() { printf '{"is_error":false,"result":"ok"}\n'; }
attempt=$(grep -c . "$WF_TMP/$task.attempts" 2>/dev/null || echo 0)
attempt=$((attempt + 1))
printf 'attempt %s\n' "$attempt" >>"$WF_TMP/$task.attempts"
work() {
	mkdir -p app
	printf 'final\n' >"app/$task.php"
	git add "app/$task.php"
	commit "Add the $task service"
}
case "$task" in
verb)
	workflow report started 'reading the brief' || printf 'report-started=%s\n' "$?" >>"$WF_TMP/verb.err"
	work
	workflow report ready 'done through the verb' || printf 'report-ready=%s\n' "$?" >>"$WF_TMP/verb.err"
	workflow report bogus 'not a state' 2>>"$WF_TMP/verb.err"; printf 'bogus=%s\n' "$?" >>"$WF_TMP/verb.err"
	;;
swapped)
	say started
	work
	# The state first and the time second, the way ebdify m1's auth wrote it.
	printf 'ready %s swapped fields, still merge-ready\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$status"
	;;
twice)
	# The first attempt dies leaving nothing, which is the one more try a
	# run gives; the second does the task.
	if [ "$attempt" = 1 ]; then
		done_json
		exit 0
	fi
	say started "attempt $attempt"
	work
	say ready
	;;
esac
done_json
FAKE

new_repo app
mem_register
"$MEM_BIN" project set verify true >/dev/null

cat >"$T_TMP/report.md" <<'PLAN'
# plan: report

- [ ] verb Add the verb service
      Files: app/verb.php
      Verify: true
- [ ] swapped Add the swapped service
      Files: app/swapped.php
      Verify: true
- [ ] twice Add the twice service
      Files: app/twice.php
      Verify: true
PLAN
export FAKE="$T_TMP/fake-worker.sh"
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$FAKE" {task} {worktree} {status} {session} {brief}'"'"' > {out} 2> {err} &'
export WORKFLOW_MAX_WORKERS=3 WORKFLOW_DEADLINE_MIN=0.5
run workflow run --plan-file "$T_TMP/report.md"
is "$RC" 0 'all three merge'
rundir="$XDG_STATE_HOME/workflow/runs/app/report"

## ------------------------------------------------------- the verb writes it

is "$(cat "$rundir/verb.state")" merged 'verb merged'
like "$(sed -n 2p "$rundir/verb.status")" '^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z started reading the brief$' 'the first report carries the time the verb supplied'
like "$(sed -n 3p "$rundir/verb.status")" 'Z ready done through the verb$' 'and the ready line'
like "$(cat "$WF_TMP/verb.err")" "is not a state -- one of started, progress, ready, blocked" 'a word off the list is refused naming the four'
like "$(cat "$WF_TMP/verb.err")" 'bogus=2$' 'with exit 2'
unlike "$(cat "$WF_TMP/verb.err")" 'report-' 'and the two real reports exited 0'
run workflow report ready 'from a checkout that is no run worktree'
is "$RC" 2 'from outside a run worktree the verb refuses'
like "$OUT" 'not a run worktree' 'and says so'

## ------------------------------------------------- a swapped line still reads

is "$(cat "$rundir/swapped.state")" merged 'a line with the state first and the time second merges'

## ------------------------------------------------ attempts append, never wipe

is "$(cat "$rundir/twice.state")" merged 'twice merged on its second attempt'
is "$(cat "$rundir/twice.dispatches")" 2 'after two dispatches'
like "$(sed -n 1p "$rundir/twice.status")" '^--- attempt 1 ---$' 'the file opens with the first attempt marker'
like "$(sed -n 2p "$rundir/twice.status")" '^--- attempt 2 ---$' 'the second attempt appended its own after it, wiping nothing'
like "$(sed -n 3p "$rundir/twice.status")" 'Z started attempt 2$' 'and then its lines'
like "$(tail -1 "$rundir/twice.status")" 'Z ready' 'ending in ready'
is "$(grep -c '^--- ' "$rundir/verb.status")" 1 'a task with one attempt carries one marker'

t_done
