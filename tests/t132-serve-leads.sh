#!/usr/bin/env bash
# workflow serve starts a lead session where a run cannot settle a thing
# itself: at a milestone's pickup, before its first run, on a worker's
# question, and on a task the run gave up on. The milestone's run starts only
# once the pickup lead has ended, a worker's question is answered by a lead
# and its task merges, and a task that failed twice gets one failure lead.
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"
mkdir -p "$XDG_CONFIG_HOME/qshell"
printf 'here\n' >"$XDG_CONFIG_HOME/qshell/machine"

# The fake worker plays the lead when its brief's GOAL names a pickup, a
# question or a failure, and a worker otherwise: ask asks once and then
# commits, die ends twice without a word.
write_exec "$T_TMP/fake-worker.sh" <<'FAKE'
#!/bin/sh
task=$1; status=$3; brief=$4
say() { printf '%s %s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" "${2:-}" >>"$status"; }
done_json() { printf '{"is_error":false,"result":"ok"}\n'; }
goal=$(sed -n '/^## GOAL/,/^## SCOPE/p' "$brief")
case "$goal" in
*'Pick up the plan'*)
	printf 'pickup\n' >>"$WF_TMP/leads.log"
	sleep 2
	touch "$WF_TMP/pickup.ended"
	done_json
	exit 0
	;;
*'Answer question #'*)
	id=$(printf '%s\n' "$goal" | sed -n 's/.*Answer question #\([A-Za-z0-9]*\).*/\1/p' | head -n 1)
	printf 'question %s\n' "$id" >>"$WF_TMP/leads.log"
	mem answer "$id" 'yes, add it' >/dev/null
	done_json
	exit 0
	;;
*'second failure'*)
	t=$(printf '%s\n' "$goal" | sed -n "s/.*Settle task \([a-z0-9]*\)'s second failure.*/\1/p" | head -n 1)
	printf 'failure %s\n' "$t" >>"$WF_TMP/leads.log"
	done_json
	exit 0
	;;
esac
[ -e "$WF_TMP/pickup.ended" ] && printf 'after\n' >>"$WF_TMP/$task.saw" || printf 'before\n' >>"$WF_TMP/$task.saw"
attempt=$(grep -c . "$WF_TMP/$task.attempts" 2>/dev/null || echo 0)
attempt=$((attempt + 1))
printf 'attempt %s\n' "$attempt" >>"$WF_TMP/$task.attempts"
case "$task" in
ask)
	say started
	if [ "$attempt" = 1 ]; then
		mem ask 'may I add the ask service beside the cart?' >"$WF_TMP/ask-id"
		say blocked "asked $(cat "$WF_TMP/ask-id")"
		done_json
		exit 0
	fi
	printf 'ask\n' >app/ask.php
	git add app/ask.php
	git -c core.hooksPath=/dev/null commit -qm "Add the ask service"
	say ready
	done_json
	;;
die)
	done_json
	;;
esac
FAKE
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$WF_TMP/fake-worker.sh" {task} {worktree} {status} {brief}'"'"' > {out} 2> {err} &'

new_repo app
mem_register
"$MEM_BIN" project set verify true >/dev/null
mkdir -p app
printf 'cart\n' >app/Cart.php
git add app/Cart.php
git -c core.hooksPath=/dev/null commit -qm 'Add the cart model'
printf '# roadmap: app-road\n\n- [ ] m1 The first milestone\n' | "$MEM_BIN" roadmap --stdin >/dev/null
"$MEM_BIN" roadmap --status approved >/dev/null
"$MEM_BIN" plan m1 --stdin >/dev/null <<-EOF
# plan: m1

- [ ] ask Add the ask service
      Files: app/*.php
      Verify: true
- [ ] die Add the die service
      Files: lib/die.php
      Verify: true
EOF

S="$XDG_STATE_HOME/workflow/serve/app"
R="$XDG_STATE_HOME/workflow/runs/app/m1"
leads() { grep -c "^$1" "$T_TMP/leads.log" 2>/dev/null; }

cd "$T_TMP" || exit 1
WORKFLOW_DEADLINE_MIN=0.5 workflow serve --tick 1 >"$T_TMP/serve.out" 2>&1 &
serve_pid=$!
trap 'st=$?; kill "$serve_pid" 2>/dev/null; wait "$serve_pid" 2>/dev/null; (exit $st); t_done' EXIT

for _ in $(seq 120); do
	[ "$(cat "$S/stage" 2>/dev/null)" = waiting ] && [ "$(leads failure)" = 1 ] && break
	sleep 1
done
# A few more ticks, for a lead served once to be started again.
sleep 4

## ------------------------------------------------------------- the pickup

is "$(leads pickup)" 1 'one pickup lead for the milestone'
brief=$(cat "$S/m1.pickup.md" 2>/dev/null)
like "$brief" 'Add the cart model' 'the pickup brief carries the git log'
like "$brief" '`app/\*\.php`: `app/Cart\.php`' 'and a Files glob with the paths it matches today'
is "$(sort -u "$T_TMP/ask.saw" 2>/dev/null)" after 'the run started only after the pickup lead ended'
is "$(sort -u "$T_TMP/die.saw" 2>/dev/null)" after 'for every task'

## ----------------------------------------------------------- the question

id=$(sed 's/^#//' "$T_TMP/ask-id" 2>/dev/null)
is "$(leads question)" 1 'one question lead'
like "$(cat "$T_TMP/leads.log")" "question $id" 'for the worker'"'"'s question'
answer=$("$MEM_BIN" --project app questions --for orchestrator --json | python3 -c '
import json,sys
for q in json.load(sys.stdin)["questions"]:
    if q["task"] == "m1/ask": print(q["answer"])')
is "$answer" 'yes, add it' 'the lead answered it'
is "$(cat "$R/ask.dispatches" 2>/dev/null)" 2 'the run sent the task back with the answer'
is "$(cat "$R/ask.state" 2>/dev/null)" merged 'and it merged'

## --------------------------------------------------- a task failed twice

is "$(cat "$R/die.dispatches" 2>/dev/null)" 2 'the task was dispatched twice'
is "$(cat "$R/die.state" 2>/dev/null)" failed 'and failed'
is "$(leads failure)" 1 'one failure lead for it'
like "$(cat "$T_TMP/leads.log")" 'failure die' 'naming the task'
[ -s "$S/leads" ] && notok 'no lead is left recorded' "$(cat "$S/leads")" || ok 'no lead is left recorded'

## ------------------------------------------- a takeover with old failures

# A serve taking over a run reads its events from the start. Of three
# tasks the events say failed, two have merged since, and only the one
# still failed gets a lead.
kill "$serve_pid" 2>/dev/null
wait "$serve_pid" 2>/dev/null
grep -v ' question ' "$R/events" >"$T_TMP/events"
{
	cat "$T_TMP/events"
	printf '2026-10-03T10:00:00Z failed ask -- the worker stopped without reporting ready\n'
	printf '2026-10-03T10:00:01Z failed old -- the worker stopped without reporting ready\n'
} >"$R/events"
printf 'merged\n' >"$R/old.state"
rm -f "$S/events.cursor" "$S/served"
before=$(leads failure)
WORKFLOW_DEADLINE_MIN=0.5 workflow serve --tick 1 >>"$T_TMP/serve.out" 2>&1 &
serve_pid=$!
for _ in $(seq 60); do
	[ "$(cat "$S/events.cursor" 2>/dev/null)" = "m1 $(stat -c %s "$R/events")" ] && break
	sleep 1
done
sleep 3

is "$(cat "$S/events.cursor" 2>/dev/null)" "m1 $(stat -c %s "$R/events")" 'serve read every event'
is "$(leads failure)" "$((before + 1))" 'one failure lead on the takeover'
is "$(grep '^failure' "$T_TMP/leads.log" | tail -n 1)" 'failure die' 'for the task still failed'
is "$(cat "$S/served" 2>/dev/null)" 'failed die 2' 'and served holds only its key'
