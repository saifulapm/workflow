#!/usr/bin/env bash
# A red trunk is refused at the door. Before the first dispatch the run puts
# the commit it starts from through the gate's own suite, unless the gate
# already proved that commit green (every green gate records the merged
# commit); red means exit 2 with the failing check named and nothing
# dispatched, since a run on a red trunk fails every task at its gate, each
# after a whole worker.
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"
write_exec "$T_TMP/worker.sh" <<'FAKE'
#!/bin/sh
task=$1; status=$3
printf '%s started\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$status"
mkdir -p app
printf '%s\n' "$task" >"app/$task.php"
git add "app/$task.php"
git -c core.hooksPath=/dev/null commit -qm "Add a service"
printf '%s ready\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$status"
printf '{"is_error":false,"result":"ok"}\n'
FAKE
export FAKE="$T_TMP/worker.sh"
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$FAKE" {task} {worktree} {status} {session}'"'"' > {out} 2> {err} &'
export WORKFLOW_MAX_WORKERS=2 WORKFLOW_DEADLINE_MIN=0.5

new_repo app
mem_register
# The suite passes only once the trunk carries `ok`.
"$MEM_BIN" project set verify 'test -f ok' >/dev/null

plan() {
	cat >"$T_TMP/$1.md" <<-PLAN
	# plan: $1

	- [ ] t1 Add the t1 service
	      Files: app/t1.php
	      Verify: true
	- [ ] t2 Add the t2 service
	      Files: app/t2.php
	      Verify: true
	PLAN
}

## --------------------------------------------------------- red: refused

plan red
run workflow run --plan-file "$T_TMP/red.md"
is "$RC" 2 'a red trunk is refused before anything is dispatched'
like "$OUT" 'the gate has not seen [0-9a-f]{12} green -- running the suite once before the first dispatch' 'the run says it is checking'
like "$OUT" 'the trunk is red before anything is dispatched -- the suite is red once the change sits on integration' 'and that it is red'
like "$OUT" 'fix the trunk first: a run on a red trunk fails every task at its gate' 'and what to do'
rundir="$XDG_STATE_HOME/workflow/runs/app/red"
[ -e "$rundir/t1.state" ] && notok 'nothing was dispatched' "$(cat "$rundir/t1.state")" || ok 'nothing was dispatched'
truthy "$([ -f "$rundir/base.gate" ] && echo 0 || echo 1)" 'the suite output is kept as base.gate'
truthy "$([ ! -d "$XDG_STATE_HOME/workflow/worktrees/app/red" ] && echo 0 || echo 1)" 'and the worktrees were taken down again'
is "$(ls "$XDG_STATE_HOME/workflow/green" 2>/dev/null | wc -l)" 0 'nothing is recorded green'
like "$("$MEM_BIN" log --type run --json)" 'run red: refused, the trunk is red' 'the log carries the refusal'

## ----------------------------------------- red once: the second run decides

# One timed-out test refused a trunk that was green alone (frictions
# #4TKKYYFT, #CC4DKNS9): red is run once more, as a task's gate is.
export TRUNK_RUNS="$T_TMP/trunk-runs"
write_exec "$T_TMP/flaky.sh" <<'FAKE'
#!/bin/sh
n=$(cat "$TRUNK_RUNS" 2>/dev/null || echo 0)
n=$((n + 1))
printf '%s\n' "$n" >"$TRUNK_RUNS"
if [ "$n" = 1 ]; then
	printf 'test view::card ... FAILED\n'
	exit 1
fi
exit 0
FAKE
"$MEM_BIN" project set verify "$T_TMP/flaky.sh" >/dev/null
cat >"$T_TMP/flaky.md" <<-PLAN
	# plan: flaky

	- [ ] f1 Add the f1 service
	      Files: app/f1.php
	      Verify: true
	- [ ] f2 Add the f2 service
	      Files: app/f2.php
	      Verify: true
	PLAN
run workflow run --plan-file "$T_TMP/flaky.md"
is "$RC" 0 'a trunk red once and green on the second run goes ahead'
like "$OUT" 'run flaky: the trunk was red once and green on the second run -- see .*base\.gate\.1' \
	'and says the suite needed two goes'
like "$(cat "$XDG_STATE_HOME/workflow/runs/app/flaky/base.gate.1")" 'view::card' "the red run's output is kept"
"$MEM_BIN" project set verify 'test -f ok' >/dev/null

## -------------------------------------------------- green: once, then not

: >ok
git add ok
git -c core.hooksPath=/dev/null commit -qm 'Make the suite pass'
plan green
run workflow run --plan-file "$T_TMP/green.md"
is "$RC" 0 'with the trunk green the run goes ahead'
like "$OUT" 'running the suite once before the first dispatch' 'after one suite on the trunk'
gdir="$XDG_STATE_HOME/workflow/runs/app/green"
is "$(cat "$gdir/t1.state")" merged 'and merges'
tree=$(git rev-parse 'integration/green^{tree}')
truthy "$(ls "$XDG_STATE_HOME"/workflow/green/*/ | grep -qx "$tree" && echo 0 || echo 1)" \
	'the last green gate recorded the tree it proved, in verify'"'"'s own cache'

# A second plan from the trunk the last gate merged onto: the gate has seen
# that tree, so no suite runs before the first dispatch.
git merge -q --ff-only integration/green

# And a green earned on another tree of the same project first -- a task's
# own gate, a pre-commit hook -- which used to overwrite the one above and
# cost this run a whole suite on a trunk the gate had just proved (friction
# #700H11G1). The cache is a set: both trees are green at once.
: >unrelated
git add unrelated
other=$(git write-tree)
run workflow verify --hook
is "$RC" 0 'an unrelated tree of the same project verifies green too'
git rm -q --cached unrelated
rm -f unrelated
truthy "$(ls "$XDG_STATE_HOME"/workflow/green/*/ | grep -qx "$other" && echo 0 || echo 1)" \
	'and is recorded'
truthy "$(ls "$XDG_STATE_HOME"/workflow/green/*/ | grep -qx "$tree" && echo 0 || echo 1)" \
	'beside the trunk the gate proved, not over it'

cat >"$T_TMP/again.md" <<-PLAN
	# plan: again

	- [ ] t3 Add the t3 service
	      Files: app/t3.php
	      Verify: true
	- [ ] t4 Add the t4 service
	      Files: app/t4.php
	      Verify: true
	PLAN
run workflow run --plan-file "$T_TMP/again.md"
is "$RC" 0 'a plan from the recorded commit runs'
unlike "$OUT" 'running the suite once before the first dispatch' 'without a suite on the trunk: the gate already proved it'


## ------------------------------------------- hung: stopped at the deadline

# A suite that never ends used to hold the run for as long as it hung (a
# mem test waiting on a FIFO held a trunk check 36 minutes, #96ZY7438). The
# gate has a deadline of its own; past it the whole process group goes.
"$MEM_BIN" project set verify 'sleep 31.7; test -f ok' >/dev/null
printf 'x\n' >hung
git add hung
git -c core.hooksPath=/dev/null commit -qm 'Add the hung marker'
plan hung
start=$(date +%s)
run env WORKFLOW_GATE_MIN=0.05 workflow run --plan-file "$T_TMP/hung.md"
took=$(($(date +%s) - start))
is "$RC" 2 'a suite past the gate deadline is a red trunk'
like "$OUT" 'the gate ran past its 3 s deadline and was stopped' 'and says it was stopped, with the deadline'
truthy "$([ "$took" -lt 25 ] && echo 0 || echo 1)" "the run did not wait the suite out (took ${took}s)"
truthy "$(pgrep -f 'sleep 31.7' >/dev/null && echo 1 || echo 0)" 'and nothing of the suite is left running'

## -------------------------------------- empty: nothing to prove, nothing refused

# A first commit with no suite: the plan's first task adds one. The trunk
# check has nothing to run and used to hard-fail the run on it (#NKBRS393).
new_repo empty
mem_register
write_exec "$T_TMP/bootstrap.sh" <<'FAKE'
#!/bin/sh
task=$1; status=$3
printf '%s started\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$status"
if [ "$task" = s1 ]; then
	printf 'test:\n\ttrue\n' >Makefile
	git add Makefile
else
	printf '%s\n' "$task" >"$task.txt"
	git add "$task.txt"
fi
git -c core.hooksPath=/dev/null commit -qm "Add a file"
printf '%s ready\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$status"
printf '{"is_error":false,"result":"ok"}\n'
FAKE
cat >"$T_TMP/empty.md" <<-PLAN
	# plan: empty

	- [ ] s1 Add the suite
	      Files: Makefile
	      Verify: true
	- [ ] s2 Add the first thing it proves [after: s1]
	      Files: s2.txt
	      Verify: true
	PLAN
run env FAKE="$T_TMP/bootstrap.sh" workflow run --plan-file "$T_TMP/empty.md"
is "$RC" 0 'a plan whose first task adds the suite runs on an empty repository'
like "$OUT" 'has no suite to run yet -- nothing to prove before the first dispatch' 'and the run says why it checked nothing'
is "$(cat "$XDG_STATE_HOME/workflow/runs/empty/empty/s2.state")" merged 'both tasks merge, each gated on the suite s1 added'

t_done
