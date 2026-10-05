#!/usr/bin/env bash
# Serve starts a project's sessions in the checkout its runs use. mem lists a
# project's checkouts sorted by path, so with two clones of one project the
# first listed is whichever sorts first; a run writes the top of its work
# tree into `<run dir>/checkout`, and serve takes the newest such file. The
# walk and a later failure lead of a project run by hand in the clone that
# sorts second start there; a project with no run yet, and one whose
# `checkout` names a directory since removed, use the first listed.
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"
unset WORKFLOW_STATUS_FILE
mkdir -p "$XDG_CONFIG_HOME/qshell"
printf 'here\n' >"$XDG_CONFIG_HOME/qshell/machine"

# The fake session keeps the directory it was started in: a walk's in
# walk.pwd and passes, a lead's in lead.<project>.pwd. A task commits its
# file.
write_exec "$T_TMP/fake-worker.sh" <<'FAKE'
#!/bin/sh
task=$1; status=$3; brief=$4
done_json() { printf '{"is_error":false,"result":"ok"}\n'; }
goal=$(sed -n '/^## GOAL/,/^## SCOPE/p' "$brief")
case "$goal" in
*'Walk the Show path'*)
	pwd >"$WF_TMP/walk.pwd"
	workflow report ready pass
	done_json
	exit 0
	;;
*'of the lead skill'*)
	pwd >"$WF_TMP/lead.$MEM_PROJECT.pwd"
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

# project <name> <road>: a checkout at b/<name> with an origin, its roadmap
# <road> approved and m1's plan stored, and a clone of it at a/<name> that
# mem knows as the same project by its origin.
project() {
	git init -q --bare "$T_TMP/$1.git"
	new_repo "b/$1"
	git remote add origin "$T_TMP/$1.git"
	git push -q origin HEAD:main
	mem_register
	"$MEM_BIN" project set verify true >/dev/null
	printf '%s' "$2" | "$MEM_BIN" roadmap --stdin >/dev/null
	"$MEM_BIN" roadmap --status approved >/dev/null
	"$MEM_BIN" plan m1 --stdin >/dev/null <<-EOF
	# plan: m1

	- [ ] a1 Add the a1 service
	      Files: app/a1.php
	      Verify: true
	EOF
	"$MEM_BIN" plan --from m1 >/dev/null
	mkdir -p "$T_TMP/a"
	git clone -q "$T_TMP/$1.git" "$T_TMP/a/$1"
	(cd "$T_TMP/a/$1" && mem_register)
}

project app '# roadmap: app-road

- [ ] m1 The home page
      Surface: web
      Show: open the home page, the heading reads Hello
- [ ] m2 The about page
'
project fresh '# roadmap: fresh-road

- [ ] m1 The home page
'
project moved '# roadmap: moved-road

- [ ] m1 The home page
'

is "$("$MEM_BIN" projects --json | jq -r '.projects[] | select(.name == "app") | .checkouts | join(" ")')" \
	"$T_TMP/a/app $T_TMP/b/app" 'mem lists the clone that sorts first first'

cd "$T_TMP/b/app" || exit 1
run env WORKFLOW_DEADLINE_MIN=0.5 workflow run
like "$OUT" 'milestone m1 landed; the roadmap tick waits' 'a run by hand in the second clone lands m1'
R="$XDG_STATE_HOME/workflow/runs"
is "$(cat "$R/app/m1/checkout" 2>/dev/null)" "$T_TMP/b/app" 'the run writes the checkout it ran in'

# A checkout file naming a clone that has since gone.
mkdir -p "$R/moved/m1" "$T_TMP/c/moved"
printf '%s\n' "$T_TMP/c/moved" >"$R/moved/m1/checkout"
rmdir "$T_TMP/c/moved"

S="$XDG_STATE_HOME/workflow/serve"
# tick_until <condition>: tick serve once a second until the condition holds.
tick_until() {
	for _ in $(seq 90); do
		eval "$1" && return 0
		WORKFLOW_DEADLINE_MIN=0.5 workflow serve --once >>"$T_TMP/serve.out" 2>&1
		sleep 1
	done
	eval "$1"
}

cd "$T_TMP" || exit 1
trap 'st=$?; kill "$(cat "$S/app/child.pid" 2>/dev/null)" 2>/dev/null; (exit $st); t_done' EXIT

## ------------------------------------------------------ no run, a gone one

tick_until '[ -s "$T_TMP/lead.fresh.pwd" ] && [ -s "$T_TMP/lead.moved.pwd" ]' ||
	notok 'the pickup leads start' "$(tail -5 "$T_TMP/serve.out")"
is "$(cat "$T_TMP/lead.fresh.pwd" 2>/dev/null)" "$T_TMP/a/fresh" 'a project with no run yet uses the first listed'
is "$(cat "$T_TMP/lead.moved.pwd" 2>/dev/null)" "$T_TMP/a/moved" 'a checkout since removed falls back to the first listed'

## --------------------------------------------------------------- the walk

tick_until '[ -s "$T_TMP/walk.pwd" ]' || notok 'the walk starts' "$(tail -5 "$T_TMP/serve.out")"
is "$(cat "$T_TMP/walk.pwd" 2>/dev/null)" "$T_TMP/b/app" 'the walk starts in the clone the run used'
tick_until '"$MEM_BIN" --project app roadmap | grep -q "^- \[x\] m1 "'
like "$("$MEM_BIN" --project app roadmap)" '^- \[x\] m1 ' 'its pass ticks the milestone'

## ------------------------------------------------------ a failure lead

tick_until '[ "$(cat "$S/app/stage" 2>/dev/null)" = needs-plan ]'
is "$(cat "$S/app/stage" 2>/dev/null)" needs-plan 'the next milestone waits for a plan'
[ -e "$T_TMP/lead.app.pwd" ] && notok 'no lead before the failure' "$(cat "$T_TMP/lead.app.pwd")" || ok 'no lead before the failure'
# A failure written the way the run writes one: the state first, then the
# event.
mkdir -p "$R/app/m2"
printf 'failed\n' >"$R/app/m2/x1.state"
printf '1\n' >"$R/app/m2/x1.dispatches"
printf '%s failed x1 -- the worker stopped without reporting ready\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >"$R/app/m2/events"
tick_until '[ -s "$T_TMP/lead.app.pwd" ]' || notok 'the failure lead starts' "$(tail -5 "$T_TMP/serve.out")"
is "$(cat "$T_TMP/lead.app.pwd" 2>/dev/null)" "$T_TMP/b/app" 'the failure lead starts there too'
