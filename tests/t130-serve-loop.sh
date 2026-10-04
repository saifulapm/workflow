#!/usr/bin/env bash
# workflow serve ticks over every project with a checkout on this machine and
# runs its roadmap milestone after milestone, one child run at a time. A
# two-milestone roadmap ends ticked and in maintenance with its files on the
# trunk, a paused project starts nothing, and a run that stops short leaves its
# project waiting with no second run until something about it changes. A
# milestone a run by hand landed is ticked by serve with no pickup, and a
# milestone with no stored plan starts nothing and says so once.
source "$(dirname -- "$0")/lib.sh"
t_init

mkdir -p "$XDG_CONFIG_HOME/qshell"
printf 'here\n' >"$XDG_CONFIG_HOME/qshell/machine"

# The fake worker: one commit per task, except sulk, which reports blocked.
# Serve's leads go out through the same command; one does nothing here.
write_exec "$T_TMP/fake-worker.sh" <<'FAKE'
#!/bin/sh
task=$1; status=$3; brief=$4
if sed -n '/^## GOAL/,/^## SCOPE/p' "$brief" 2>/dev/null | grep -q 'of the lead skill'; then
	printf '{"is_error":false,"result":"ok"}\n'
	exit 0
fi
say() { printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" >>"$status"; }
say started
if [ "$task" = sulk ]; then
	say 'blocked I would rather not'
	printf '{"is_error":false,"result":"ok"}\n'
	exit 0
fi
mkdir -p app
printf '%s\n' "$task" >"app/$task.php"
git add "app/$task.php"
git -c core.hooksPath=/dev/null commit -qm "Add a service"
say ready
printf '{"is_error":false,"result":"ok"}\n'
FAKE
export FAKE="$T_TMP/fake-worker.sh"
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$FAKE" {task} {worktree} {status} {brief}'"'"' > {out} 2> {err} &'

# project <name> <slug>... -- a checkout mem knows, its suite `true`, and an
# approved roadmap of those milestones.
project() {
	local name=$1 slug
	shift
	new_repo "$name"
	mem_register
	"$MEM_BIN" project set verify true >/dev/null
	{
		printf '# roadmap: %s-road\n\n' "$name"
		for slug in "$@"; do printf -- '- [ ] %s The %s milestone\n' "$slug" "$slug"; done
	} | "$MEM_BIN" roadmap --stdin >/dev/null
	"$MEM_BIN" roadmap --status approved >/dev/null
}

# milestone <slug> <task> <task> -- a two-task plan stored under the slug.
milestone() {
	"$MEM_BIN" plan "$1" --stdin >/dev/null <<-EOF
	# plan: $1

	- [ ] $2 Add the $2 service
	      Files: app/$2.php
	      Verify: true
	- [ ] $3 Add the $3 service
	      Files: app/$3.php
	      Verify: true
	EOF
}

project app m1 m2
milestone m1 a1 a2
milestone m2 b1 b2

project idle i1
milestone i1 c1 c2
"$MEM_BIN" project set paused 'here 2026-10-03' >/dev/null

project sulky s1
milestone s1 w1 sulk

# A milestone with no stored plan, and a plan of record that is not its own.
project planless p1

# A milestone a run by hand lands before serve starts: the run leaves the
# roadmap to serve.
project byhand h1
milestone h1 d1 d2
"$MEM_BIN" plan --from h1 >/dev/null
run env WORKFLOW_DEADLINE_MIN=0.5 workflow run
is "$RC" 0 'a run by hand lands its milestone'
like "$OUT" 'milestone h1 landed; the roadmap tick waits' 'and says serve ticks it'
like "$("$MEM_BIN" roadmap)" '^- \[ \] h1 ' 'leaving it open in the roadmap'

S="$XDG_STATE_HOME/workflow/serve"
stage() { cat "$S/$1/stage" 2>/dev/null; }
starts() { grep -c 'tasks, up to' "$S/$1/run.log" 2>/dev/null; }

## ------------------------------------------------- serve, left to itself

cd "$T_TMP" || exit 1
WORKFLOW_DEADLINE_MIN=0.5 workflow serve --tick 1 >"$T_TMP/serve.out" 2>&1 &
serve_pid=$!
trap 'st=$?; kill "$serve_pid" 2>/dev/null; wait "$serve_pid" 2>/dev/null; (exit $st); t_done' EXIT

sleep 1
run workflow serve --once
is "$RC" 2 'a second serve on this machine is refused'
like "$OUT" 'another serve is live on this machine' 'and says why'

for _ in $(seq 120); do
	[ "$(stage app)" = maintenance ] && [ "$(stage sulky)" = waiting ] &&
		[ "$(stage byhand)" = maintenance ] && break
	sleep 1
done
# A few more ticks, for a waiting project to start a run it should not.
sleep 4

## --------------------------------------- two milestones, one after the other

is "$(stage app)" maintenance 'the two-milestone project ends in maintenance'
run_out "$MEM_BIN" --project app roadmap
like "$OUT" '^- \[x\] m1 ' 'the first milestone is ticked'
like "$OUT" '^- \[x\] m2 ' 'and the second'
is "$("$MEM_BIN" --project app roadmap --status)" maintenance 'the roadmap reads maintenance'
is "$(starts app)" 2 'one run per milestone'
for f in a1 a2 b1 b2; do
	is "$(git -C "$T_TMP/app" show "HEAD:app/$f.php" 2>&1)" "$f" "$f's file is on the trunk"
done
head=$(git -C "$T_TMP/app" rev-parse HEAD)
is "$(cat "$S/app/last-landed" 2>/dev/null)" "$head" 'last-landed is the trunk head'
is "$("$MEM_BIN" --project app plan --status)" done 'the last plan is done'
runs=$("$MEM_BIN" --project app log --type run --limit 200 --json)
like "$runs" 'dogfood m1: skipped, m5' 'the dogfood stage ran for the first milestone'
like "$runs" 'dogfood m2: skipped, m5' 'and for the second'
like "$runs" 'hygiene m1: [0-9]+ findings' 'the hygiene count is logged for the first'
like "$runs" 'hygiene m2: [0-9]+ findings' 'and for the second'
run_out "$MEM_BIN" --project app status
like "$OUT" "m2 landed at ${head:0:7} on [0-9]{4}-[0-9]{2}-[0-9]{2}" 'the status line names the milestone, the commit and the date'
run_out "$MEM_BIN" --project app handoff
like "$OUT" "m2 landed at ${head:0:7}" 'the handoff says what landed'
like "$OUT" 'no milestone left' 'and that nothing is next'
like "$("$MEM_BIN" --project app project current --json)" '"runner":"here"' \
	'serve holds the claim after the run let it go'
[ -e "$S/app/milestone" ] && notok 'no milestone is open' "$(cat "$S/app/milestone")" || ok 'no milestone is open'

## ------------------------------------------------- a paused project

is "$(stage idle)" paused 'the paused project reads paused'
[ -e "$S/idle/run.log" ] && notok 'and no run started there' "$(cat "$S/idle/run.log")" || ok 'and no run started there'
[ -d "$XDG_STATE_HOME/workflow/runs/idle" ] && notok 'nor a run dir made' || ok 'nor a run dir made'
is "$("$MEM_BIN" --project idle roadmap --status)" approved 'its roadmap is left approved'

## ---------------------------------------- a run that stops short waits

is "$(stage sulky)" waiting 'a failed task leaves the project waiting'
is "$(starts sulky)" 1 'with no second run'
is "$(cat "$S/sulky/milestone" 2>/dev/null)" 's1 1 1' 'the open milestone is named with its place'
like "$("$MEM_BIN" --project sulky roadmap)" '^- \[ \] s1 ' 'and stays open'
like "$("$MEM_BIN" --project sulky log --type run --limit 50 --json)" 'stopped short' \
	'the stopped-short line is logged as the reason'

# Something changed: the plan of record's text. The run goes again.
"$MEM_BIN" --project sulky plan | sed 's/Add the sulk service/Add the sulk service, again/' >"$T_TMP/sulky.md"
"$MEM_BIN" --project sulky plan --stdin <"$T_TMP/sulky.md" >/dev/null
for _ in $(seq 60); do
	[ "$(starts sulky)" = 2 ] && [ "$(stage sulky)" = waiting ] && break
	sleep 1
done
is "$(starts sulky)" 2 'a changed plan starts the run again'
is "$(stage sulky)" waiting 'and it waits again when the task fails again'

## ---------------------------------------- a milestone with no stored plan

is "$(stage planless)" needs-plan 'a milestone with no stored plan reads needs-plan'
[ -e "$S/planless/p1.pickup.md" ] && notok 'and no pickup lead went out' || ok 'and no pickup lead went out'
[ -e "$S/planless/run.log" ] && notok 'nor a run started' "$(cat "$S/planless/run.log")" || ok 'nor a run started'
is "$(grep -o 'serve p1: no stored plan -- cut one with the plan skill; nothing starts until it is stored' \
	<<<"$("$MEM_BIN" --project planless log --type run --limit 50 --json)" | wc -l)" 1 \
	'the line is logged once over every tick'
like "$("$MEM_BIN" --project planless roadmap)" '^- \[ \] p1 ' 'and the milestone stays open'

## ----------------------------------- a milestone landed by a run by hand

like "$("$MEM_BIN" --project byhand roadmap)" '^- \[x\] h1 ' 'serve ticks the milestone a run by hand landed'
[ -e "$S/byhand/h1.pickup.md" ] && notok 'with no pickup lead' || ok 'with no pickup lead'
is "$(starts byhand)" 1 'its one run found nothing left to do'
like "$("$MEM_BIN" --project byhand log --type run --limit 50 --json)" 'dogfood h1: skipped, m5' \
	'and the milestone end ran after it'
