#!/usr/bin/env bash
# A project whose dogfood-machine is another machine is walked there: at the
# milestone end near posts the request for the trunk head and the steps,
# writes `far <question id>` into its walk file and holds the stage dogfood
# with no session of its own. Far's serve, its own home over the same mem
# store, walks it in its own checkout and answers; near reads the answer as
# it reads a report. A pass ticks the milestone. A checkout on far without
# the commit walks nothing: its one finding names the push, near logs it as
# `findings 1` and ticks nothing. Neither serve fetches, pulls or pushes.
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"
unset WORKFLOW_STATUS_FILE
mkdir -p "$XDG_CONFIG_HOME/qshell"
printf 'near\n' >"$XDG_CONFIG_HOME/qshell/machine"

# Far keeps its own home, state and machine name, and reads and writes the
# store and index near does.
FAR="$T_TMP/far-home"
mkdir -p "$FAR/.config/qshell" "$FAR/.local/state"
printf 'far\n' >"$FAR/.config/qshell/machine"
far() { env HOME="$FAR" XDG_STATE_HOME="$FAR/.local/state" XDG_CONFIG_HOME="$FAR/.config" "$@"; }

# The fake session: each start is logged with the machine it ran on. A walk
# passes, a lead does nothing, a task commits its file.
write_exec "$T_TMP/fake-worker.sh" <<'FAKE'
#!/bin/sh
task=$1; status=$3; brief=$4
here=$(cat "$XDG_CONFIG_HOME/qshell/machine")
done_json() { printf '{"is_error":false,"result":"ok"}\n'; }
goal=$(sed -n '/^## GOAL/,/^## SCOPE/p' "$brief")
case "$goal" in
*'Walk the Show path'*)
	printf 'dogfood %s %s\n' "$here" "$MEM_PROJECT" >>"$WF_TMP/sessions.log"
	workflow report ready pass
	done_json
	exit 0
	;;
*'of the lead skill'*)
	printf 'lead %s %s\n' "$here" "$MEM_PROJECT" >>"$WF_TMP/sessions.log"
	done_json
	exit 0
	;;
esac
say() { printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" >>"$status"; }
say started
mkdir -p app
printf '%s\n' "$task" >"app/$task.php"
git add "app/$task.php"
git -c core.hooksPath=/dev/null commit -qm "Add a service"
say ready
done_json
FAKE
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$WF_TMP/fake-worker.sh" {task} {worktree} {status} {brief}'"'"' > {out} 2> {err} &'

# project <name>: a checkout on near whose one milestone a run by hand landed,
# its dogfood-machine far, and a clone of it on far that mem knows as the
# same project by its origin.
project() {
	git init -q --bare "$T_TMP/$1.git"
	new_repo "$1"
	git remote add origin "$T_TMP/$1.git"
	mem_register
	"$MEM_BIN" project set verify true >/dev/null
	"$MEM_BIN" project set dogfood-machine far >/dev/null
	printf '# roadmap: %s-road\n\n- [ ] m1 The home page\n      Surface: web\n      Show: open the home page, the heading reads Hello; then the footer shows the year\n' "$1" |
		"$MEM_BIN" roadmap --stdin >/dev/null
	"$MEM_BIN" roadmap --status approved >/dev/null
	"$MEM_BIN" plan m1 --stdin >/dev/null <<-EOF
	# plan: m1

	- [ ] a1 Add the a1 service
	      Files: app/a1.php
	      Verify: true
	- [ ] a2 Add the a2 service
	      Files: app/a2.php
	      Verify: true
	EOF
	"$MEM_BIN" plan --from m1 >/dev/null
	run env WORKFLOW_DEADLINE_MIN=0.5 workflow run
	like "$OUT" 'milestone m1 landed; the roadmap tick waits' "a run by hand lands $1's milestone"
	git push -q origin HEAD:main
	mkdir -p "$T_TMP/far"
	git clone -q "$T_TMP/$1.git" "$T_TMP/far/$1"
	(cd "$T_TMP/far/$1" && far "$MEM_BIN" log 'registered by the test harness' >/dev/null 2>&1)
}

project app
app_head=$(git rev-parse HEAD)
project gone
# Near's trunk moves on past what far has.
git -c core.hooksPath=/dev/null commit -q --allow-empty -m 'Move the trunk on'
gone_head=$(git rev-parse HEAD)

is "$(far "$MEM_BIN" --project app project current --json | jq -r '.machine')" far 'far names itself'
is "$(far "$MEM_BIN" projects --json | jq -r '.projects[] | select(.name == "app") | .checkouts[0]')" \
	"$T_TMP/far/app" 'and knows app by its own checkout'

N="$XDG_STATE_HOME/workflow/serve"
runs() { "$MEM_BIN" --project "$1" log --type run 2>/dev/null; }
sessions() { cat "$T_TMP/sessions.log" 2>/dev/null | grep -c "$1"; }
# far_id <project>: the question id near's walk file names.
far_id() { sed -n 's/^far //p' "$N/$1/walk" 2>/dev/null; }
# answer <project> <question>: its answer once there is one.
answer() {
	"$MEM_BIN" --project "$1" questions --for orchestrator --json |
		jq -r --arg id "$2" '.questions[] | select(.short_id == $id) | .answer // empty'
}
question() {
	"$MEM_BIN" --project "$1" questions --for orchestrator --json |
		jq -r --arg id "$2" '.questions[] | select(.short_id == $id) | .body'
}

cd "$T_TMP" || exit 1
GIT_TRACE="$T_TMP/near.trace" WORKFLOW_DEADLINE_MIN=0.5 workflow serve --tick 1 >"$T_TMP/near.out" 2>&1 &
near_pid=$!
far_pid=
trap 'st=$?; kill $near_pid $far_pid 2>/dev/null; wait $near_pid $far_pid 2>/dev/null; (exit $st); t_done' EXIT

for _ in $(seq 120); do
	[ -n "$(far_id app)" ] && [ -n "$(far_id gone)" ] && break
	sleep 1
done
# A few more ticks, for anything near should not start while it waits.
sleep 4

## ------------------------------------------------------ near asks far

app_q=$(far_id app)
gone_q=$(far_id gone)
like "$(head -1 "$N/app/walk" 2>/dev/null)" '^m1 1 [0-9]+ far 1 2 3$' 'the walk file names far and the steps'
is "$(question app "$app_q")" "dogfood m1 at $app_head on far steps 1,2,3" 'the request is for the trunk head and the steps'
is "$(question gone "$gone_q")" "dogfood m1 at $gone_head on far steps 1,2,3" 'for each project'
is "$(cat "$N/app/stage" 2>/dev/null)" dogfood 'the stage holds dogfood'
is "$(answer app "$app_q")" '' 'while the request is pending'
like "$("$MEM_BIN" --project app roadmap)" '^- \[ \] m1 ' 'and the milestone stays open'
is "$(sessions '^dogfood near ')" 0 'near starts no walk of its own'

## ----------------------------------------------------- far walks it

# Started as itself, not through far(), so the pid kept is the serve's.
env HOME="$FAR" XDG_STATE_HOME="$FAR/.local/state" XDG_CONFIG_HOME="$FAR/.config" \
	GIT_TRACE="$T_TMP/far.trace" workflow serve --tick 1 >"$T_TMP/far.out" 2>&1 &
far_pid=$!

for _ in $(seq 120); do
	"$MEM_BIN" --project app roadmap | grep -q '^- \[x\] m1 ' && break
	sleep 1
done
is "$(answer app "$app_q")" pass 'far answers pass'
is "$(sessions '^dogfood far app$')" 1 'after one walk of its own'
like "$("$MEM_BIN" --project app roadmap)" '^- \[x\] m1 ' 'and near ticks the milestone'
like "$(runs app)" 'dogfood m1: pass' 'beside its run line'
[ -e "$N/app/walk" ] && notok 'the walk file goes with the pass' "$(cat "$N/app/walk")" || ok 'the walk file goes with the pass'

## -------------------------------------------- far without the commit

for _ in $(seq 60); do
	[ "$(sed -n 3p "$N/gone/walk" 2>/dev/null)" = 'failed 0' ] && break
	sleep 1
done
is "$(answer gone "$gone_q")" 'failed 0' 'a checkout without the commit answers failed 0'
like "$(runs gone)" 'dogfood m1: findings 1' 'near logs its one finding'
found=$("$MEM_BIN" --project gone finding list --open --json)
is "$(jq '.items | length' <<<"$found")" 1 'one finding'
# mem cuts a long title short, so the text is read off the item's body.
body=$("$MEM_BIN" --project gone show "$(jq -r '.items[0].short_id' <<<"$found")" --json | jq -r '.items[0].body' | sed '/^$/d')
is "$body" "commit $gone_head is not on far: push it there, the engine never pushes" 'naming the commit and the push'
is "$(sessions '^dogfood .* gone$')" 0 'nothing walks it'
sleep 3
like "$("$MEM_BIN" --project gone roadmap)" '^- \[ \] m1 ' 'and nothing is ticked'
is "$(sessions '^dogfood near ')" 0 'near never walked'

## ----------------------------------------------- nothing over the wire

truthy "$([ -s "$T_TMP/near.trace" ] && [ -s "$T_TMP/far.trace" ] && echo 0 || echo 1)" 'both serves ran git'
unlike "$(cat "$T_TMP/near.trace" "$T_TMP/far.trace")" 'git[- ](fetch|pull|push)' 'and neither fetched, pulled or pushed'
