#!/usr/bin/env bash
# workflow serve over a three-milestone roadmap for WF_SOAK_MIN minutes (two by
# default; twenty once by hand before a release), its fake worker failing,
# asking or committing at seeded random (WF_SOAK_SEED) and its fake lead
# answering. Serve is stopped with SIGTERM once while workers are going and
# started again, and killed with SIGKILL, its run with it, once while a merge
# is in flight and started again. Every milestone ends ticked, every task's
# file is on the trunk once, and no task is left dispatched.
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"
mkdir -p "$XDG_CONFIG_HOME/qshell"
printf 'here\n' >"$XDG_CONFIG_HOME/qshell/machine"

min=${WF_SOAK_MIN:-2}
export WF_SEED=${WF_SOAK_SEED:-131}
# More minutes buy more tasks, so a long soak meets more of each outcome.
per=$((min * 3 / 2))
[ "$per" -ge 3 ] || per=3
printf '# WF_SOAK_MIN=%s WF_SOAK_SEED=%s, %s tasks a milestone\n' "$min" "$WF_SEED" "$per"

# A lead answers the question its brief names and ends; every other lead ends
# at once. A worker pauses one to three seconds, then on its first attempt
# dies without a word, asks, or commits, by the seed; any later attempt
# commits.
write_exec "$T_TMP/fake-worker.sh" <<'FAKE'
#!/bin/sh
task=$1; status=$3; brief=$4
say() { printf '%s %s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" "${2:-}" >>"$status"; }
done_json() { printf '{"is_error":false,"result":"ok"}\n'; }
goal=$(sed -n '/^## GOAL/,/^## SCOPE/p' "$brief")
case "$goal" in
*'Answer question #'*)
	id=$(printf '%s\n' "$goal" | sed -n 's/.*Answer question #\([A-Za-z0-9]*\).*/\1/p' | head -n 1)
	mem answer "$id" 'yes, go on' >/dev/null
	done_json
	exit 0
	;;
*'of the lead skill'*)
	done_json
	exit 0
	;;
esac
n=$(($(cat "$WF_TMP/$task.attempts" 2>/dev/null || echo 0) + 1))
printf '%s\n' "$n" >"$WF_TMP/$task.attempts"
r=$(printf '%s %s' "$WF_SEED" "$task" | cksum | cut -d' ' -f1)
sleep $((r % 3 + 1))
if [ "$n" = 1 ]; then
	case $((r / 3 % 3)) in
	0)
		printf '%s\n' "$task" >>"$WF_TMP/died"
		exit 0
		;;
	1)
		say started
		id=$(mem ask "may $task add its service?")
		printf '%s\n' "$task" >>"$WF_TMP/asked"
		say blocked "asked $id"
		done_json
		exit 0
		;;
	esac
fi
say started
mkdir -p app
printf '%s\n' "$task" >"app/$task.php"
git add "app/$task.php"
git -c core.hooksPath=/dev/null commit -qm "Add the $task service"
say ready
done_json
FAKE
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$WF_TMP/fake-worker.sh" {task} {worktree} {status} {brief}'"'"' > {out} 2> {err} &'

new_repo app
mem_register
# A gate that takes a second holds each merge's intent line open long enough
# to be caught.
"$MEM_BIN" project set verify 'sleep 1' >/dev/null
printf '# roadmap: soak\n\n- [ ] m1 The first\n- [ ] m2 The second\n- [ ] m3 The third\n' |
	"$MEM_BIN" roadmap --stdin >/dev/null
"$MEM_BIN" roadmap --status approved >/dev/null
tasks=()
for m in m1 m2 m3; do
	{
		printf '# plan: %s\n\n' "$m"
		for i in $(seq "$per"); do
			t="${m}t$i"
			tasks+=("$t")
			printf -- '- [ ] %s Add the %s service\n      Files: app/%s.php\n      Verify: true\n' "$t" "$t" "$t"
		done
	} | "$MEM_BIN" plan "$m" --stdin >/dev/null
done

S="$XDG_STATE_HOME/workflow/serve/app"
R="$XDG_STATE_HOME/workflow/runs/app"

serve() {
	WORKFLOW_DEADLINE_MIN=0.25 workflow serve --tick 1 >>"$T_TMP/serve.out" 2>&1 &
	serve_pid=$!
}
trap 'st=$?; kill "$serve_pid" 2>/dev/null; wait "$serve_pid" 2>/dev/null; (exit $st); t_done' EXIT

# A moment that loses nothing to a stop: no lead going, whose question would
# go unanswered once the next serve stops it, and no merge in flight.
quiet() {
	[ ! -s "$S/leads" ] && ! grep -qs . "$R"/*/*.merging
}

# The SIGTERM moment: a worker going, and every dispatched task on its first
# attempt, so the attempt the stop takes is one its retry gives back.
first_attempts() {
	local f t live=
	for f in "$R"/*/*.state; do
		[ "$(cat "$f" 2>/dev/null)" = dispatched ] || continue
		t=${f%.state}
		[ "$(cat "$t.dispatches" 2>/dev/null)" = 1 ] || return 1
		kill -0 "$(cat "$t.pid" 2>/dev/null)" 2>/dev/null && live=${t##*/}
	done
	[ -n "$live" ] && victim=$live
}

cd "$T_TMP" || exit 1
serve
deadline=$((SECONDS + min * 60))
termed='' killed='' victim='' merging=''
while [ "$SECONDS" -lt "$deadline" ]; do
	[ "$(cat "$S/stage" 2>/dev/null)" = maintenance ] && break
	if [ -z "$termed" ] && quiet && first_attempts; then
		termed=$SECONDS
		wpid=$(cat "$R"/*/"$victim.pid")
		kill -TERM "$serve_pid"
		wait "$serve_pid"
		term_rc=$?
		term_s=$((SECONDS - termed))
		worker_gone=$(kill -0 "$wpid" 2>/dev/null && echo alive || echo gone)
		child_gone=$(kill -0 "$(cat "$S/child.pid")" 2>/dev/null && echo alive || echo gone)
		victim_state=$(cat "$R"/*/"$victim.state")
		serve
	elif [ -n "$termed" ] && [ -z "$killed" ] && [ ! -s "$S/leads" ]; then
		for f in "$R"/*/*.merging; do
			[ -s "$f" ] || continue
			merging=${f##*/}
			merging=${merging%.merging}
			killed=$SECONDS
			kill -KILL "$serve_pid" "$(cat "$S/child.pid")"
			wait "$serve_pid" 2>/dev/null
			serve
			break
		done
	fi
	sleep 0.2
done
took=$SECONDS

## --------------------------------------------------- a SIGTERM, then a start

isnt "$termed" '' 'serve was stopped with SIGTERM while a worker was going'
is "${term_rc:-}" 0 'serve exits 0 on SIGTERM'
truthy "$([ "${term_s:-99}" -le 30 ] && echo 0)" 'within its thirty seconds'
is "${child_gone:-}" gone 'its run went before it'
is "${worker_gone:-}" gone "and stopped $victim's worker"
is "${victim_state:-}" dispatched "leaving $victim dispatched"
like "$(cat "$S/run.log")" "task $victim: (its session is gone|left dispatched by a run that is gone|the recorded session never existed)" \
	"the next serve's run adopted $victim"

## ------------------------------------------------- a SIGKILL mid-merge

isnt "$killed" '' 'serve and its run were killed while a merge was in flight'
like "$(cat "$S/run.log")" "task $merging: its merge reached .* before the run died" \
	"the next run settled $merging's interrupted merge"

## ------------------------------------------------------------ the roadmap

is "$(cat "$S/stage" 2>/dev/null)" maintenance "every milestone ticked within $min minutes (took ${took}s)"
run_out "$MEM_BIN" --project app roadmap
for m in m1 m2 m3; do
	like "$OUT" "^- \[x\] $m " "$m is ticked"
done
lost=''
for t in "${tasks[@]}"; do
	[ "$(git -C "$T_TMP/app" show "HEAD:app/$t.php" 2>&1)" = "$t" ] &&
		[ "$(git -C "$T_TMP/app" log --format=%H HEAD -- "app/$t.php" | wc -l)" = 1 ] ||
		lost+=" $t"
done
is "$lost" '' "every task's file is on the trunk once"
is "$(grep -lx dispatched "$R"/*/*.state 2>/dev/null)" '' 'no task is left dispatched'
isnt "$(cat "$T_TMP/died" 2>/dev/null)" '' 'the seed had a worker die without a word'
isnt "$(cat "$T_TMP/asked" 2>/dev/null)" '' 'and one ask'
