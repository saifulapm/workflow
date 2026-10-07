#!/usr/bin/env bash
# Every session serve starts leaves one cost line in the run log once it has
# ended: `cost <slug> <kind> <name>: key=value ...` with its minutes and the
# role it ran as. One milestone driven through its pickup lead, a worker's
# question and its walk lands with a line for each lead, each worker run and
# the walk, and serve sums every session line into one milestone line.
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"
unset WORKFLOW_STATUS_FILE
mkdir -p "$XDG_CONFIG_HOME/qshell"
printf 'here\n' >"$XDG_CONFIG_HOME/qshell/machine"

# The fake session: the pickup lead ends at once, the question lead answers
# the question, the walk reports a pass, and the one task asks once and then
# commits its file.
write_exec "$T_TMP/fake-worker.sh" <<'FAKE'
#!/bin/sh
task=$1; status=$3; brief=$4
say() { printf '%s %s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" "${2:-}" >>"$status"; }
done_json() { printf '{"is_error":false,"result":"ok"}\n'; }
goal=$(sed -n '/^## GOAL/,/^## SCOPE/p' "$brief")
case "$goal" in
*'Pick up the plan'*)
	done_json
	exit 0
	;;
*'Answer question #'*)
	id=$(printf '%s\n' "$goal" | sed -n 's/.*Answer question #\([A-Za-z0-9]*\).*/\1/p' | head -n 1)
	mem answer "$id" 'yes, add it' >/dev/null
	done_json
	exit 0
	;;
*'Walk the Show path'*)
	# What status --json lists while this walk is going, read once serve
	# has recorded the walk.
	for _ in $(seq 20); do
		workflow status --json | jq -c '.sessions' >"$WF_TMP/sessions.json"
		grep -q '"walk"' "$WF_TMP/sessions.json" && break
		sleep 0.5
	done
	workflow report ready pass
	done_json
	exit 0
	;;
esac
say started
if [ ! -e "$WF_TMP/asked" ]; then
	touch "$WF_TMP/asked"
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
FAKE
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$WF_TMP/fake-worker.sh" {task} {worktree} {status} {brief}'"'"' > {out} 2> {err} &'

new_repo app
mem_register
"$MEM_BIN" project set verify true >/dev/null
mkdir -p app
printf 'cart\n' >app/Cart.php
git add app/Cart.php
git -c core.hooksPath=/dev/null commit -qm 'Add the cart model'
printf '# roadmap: app-road\n\n- [ ] m1 The ask service\n      Show: run the ask service, it answers\n' |
	"$MEM_BIN" roadmap --stdin >/dev/null
"$MEM_BIN" roadmap --status approved >/dev/null
"$MEM_BIN" plan m1 --stdin >/dev/null <<-EOF
# plan: m1

- [ ] ask Add the ask service
      Files: app/*.php
      Verify: true
EOF

runs() { "$MEM_BIN" --project app log --type run 2>/dev/null; }

cd "$T_TMP" || exit 1
WORKFLOW_DEADLINE_MIN=0.5 workflow serve --tick 1 >"$T_TMP/serve.out" 2>&1 &
serve_pid=$!
trap 'st=$?; kill "$serve_pid" 2>/dev/null; wait "$serve_pid" 2>/dev/null; (exit $st); t_done' EXIT

for _ in $(seq 120); do
	"$MEM_BIN" --project app roadmap | grep -q '^- \[x\] m1 ' && break
	sleep 1
done
# A few more ticks, for a line written twice to show.
sleep 3

like "$("$MEM_BIN" --project app roadmap)" '^- \[x\] m1 ' 'the milestone landed'
lines=$(runs | grep ' cost m1 ')
for kind in pickup question; do
	is "$(printf '%s\n' "$lines" | grep -c " cost m1 $kind m1:")" 1 "one cost line for the $kind lead"
	like "$(printf '%s\n' "$lines" | grep " cost m1 $kind m1:")" " minutes=[0-9]+( |$)" 'with its minutes'
	like "$(printf '%s\n' "$lines" | grep " cost m1 $kind m1:")" ' role=lead$' 'and its role'
done
is "$(printf '%s\n' "$lines" | grep -c ' cost m1 walk m1:')" 1 'one cost line for the walk'
like "$(printf '%s\n' "$lines" | grep ' cost m1 walk m1:')" ' minutes=[0-9]+( |$)' 'with its minutes'
like "$(printf '%s\n' "$lines" | grep ' cost m1 walk m1:')" ' role=dogfood$' 'and its role'
is "$(jq -c '[.[] | select(.kind == "walk") | {name, minutes: (.minutes | type)}]' "$WF_TMP/sessions.json")" \
	'[{"name":"m1","minutes":"number"}]' 'status --json lists the walk going, with its minutes'
unlike "$lines" ' in=0 ' 'a session with no transcript leaves its tokens out'

# Landing sums the five sessions into one milestone line: the pickup lead,
# the worker that asked, the question lead, the worker that committed and
# the walk.
is "$(printf '%s\n' "$lines" | grep -c ' cost m1 milestone m1:')" 1 'one milestone line when it lands'
like "$(printf '%s\n' "$lines" | grep ' cost m1 milestone m1:')" ' sessions=5( |$)' 'counting its five sessions'
sessions=$(printf '%s\n' "$lines" | grep -v ' cost m1 milestone m1:')
is "$(printf '%s\n' "$sessions" | grep -c ' cost m1 ')" 5 'as many as there are session lines'
minutes=$(printf '%s\n' "$sessions" | sed -n 's/.* minutes=\([0-9]*\).*/\1/p' | awk '{ n += $1 } END { print n + 0 }')
like "$(printf '%s\n' "$lines" | grep ' cost m1 milestone m1:')" " minutes=$minutes( |$)" 'with their minutes summed'
