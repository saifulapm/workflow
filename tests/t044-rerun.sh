#!/usr/bin/env bash
# Running the same plan twice. The first run's merged work is on the
# integration branch and on no other ref, so a second run must refuse rather
# than reset it -- and must refuse before it has touched anything (review B-1).
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"

# One fake worker for every task: it commits the file its Files: line claims,
# unless a flag file tells it to reach outside and get itself parked.
write_exec "$T_TMP/fake-worker.sh" <<'FAKE'
#!/bin/sh
task=$1; status=$3
say() { printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" >>"$status"; }
commit() { git -c core.hooksPath=/dev/null commit -qm "$1"; }
say started
if [ -f "$WF_TMP/misbehave-$task" ]; then
	printf 'outside\n' >NOTOWNED.txt
	git add NOTOWNED.txt
	commit 'Reach outside the task'
else
	mkdir -p app
	printf '%s\n' "$task" >"app/$task.php"
	git add "app/$task.php"
	commit "Add the $task service"
fi
say ready
printf '{"is_error":false,"result":"ok"}\n'
FAKE

export FAKE="$T_TMP/fake-worker.sh"
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$FAKE" {task} {worktree} {status}'"'"' > {out} 2> {err} &'
export WORKFLOW_MAX_WORKERS=2 WORKFLOW_DEADLINE_MIN=0.2

php_fixture() {
	printf '{"name":"acme/app"}\n' >composer.json
	printf '#!/bin/sh\nexit 0\n' >artisan
	chmod +x artisan
	write_exec bin/php <<-'EOF'
		#!/bin/sh
		exit 0
	EOF
	git add -A
	git -c core.hooksPath=/dev/null commit -qm 'project files'
}

## ------------------------------------------------------------------ the plan

new_repo app
mem_register
php_fixture
base=$(git rev-parse HEAD)
rundir="$XDG_STATE_HOME/workflow/runs/app/rerun-check"

"$MEM_BIN" plan --stdin >/dev/null <<'EOF'
# plan: rerun-check

- [ ] t1 First service
      Files: app/t1.php
      Verify: true
- [ ] t2 Second service
      Files: app/t2.php
      Verify: true
EOF

## -------------------------------------------------------------------- run 1

: >"$T_TMP/misbehave-t2"
run workflow run
is "$RC" 1 'run 1: one task parked, so exit 1'
is "$(cat "$rundir/t1.state")" merged 'run 1: t1 merged'
is "$(cat "$rundir/t2.state")" parked 'run 1: t2 parked, having written outside its patterns'

# A park that says nothing about its own escape hatch sends whoever reads it
# hunting for the bundle (friction #BHPS3G7D).
like "$OUT" 'bundled at .*rerun-check-t2' 'run 1: the park names the bundle it wrote'
like "$OUT" 'workflow resume ' 'run 1: and the command that puts it back'
like "$(cat "$rundir/t2.bundle")" '\.bundle$' 'run 1: the bundle path is in the run dir too'

int1=$(git rev-parse integration/rerun-check)
is "$(git rev-list --count "$base..integration/rerun-check")" 1 \
	'run 1: integration carries exactly the merged task'
run git cat-file -e "integration/rerun-check:app/t1.php"
is "$RC" 0 "run 1: t1's file is on the integration branch"
like "$("$MEM_BIN" plan)" '\[x\] t1' 'run 1: the plan is ticked off for t1'

## ------------------------------- run 2: the same plan again, and it must stop

run workflow run
rerun_out=$OUT
is "$RC" 2 'run 2: refused as a configuration problem, not a failed run'
like "$rerun_out" 'integration/rerun-check holds' \
	'run 2: the message says the branch holds work an earlier run merged'
like "$rerun_out" 'git merge integration/rerun-check' \
	'run 2: and names the way to land it'
like "$rerun_out" 'rerun-check/t2' 'run 2: the leftover task branch is named too'

is "$(git rev-parse integration/rerun-check)" "$int1" 'run 2: integration is untouched'
run git merge-base --is-ancestor "$int1" integration/rerun-check
is "$RC" 0 "run 2: run 1's merged commit is still reachable"
run git cat-file -e "integration/rerun-check:app/t1.php"
is "$RC" 0 "run 2: t1's work is still on the branch"
is "$(cat "$rundir/base_sha")" "$base" 'run 2: it did not even rewrite the recorded base'
is "$(git worktree list | grep -c .)" 1 'run 2: it made no worktrees'

# Clearing only the leftover branch is not enough: the two guards are separate.
git branch -D rerun-check/t2 >/dev/null 2>&1
run workflow run
is "$RC" 2 'run 2b: still refused while integration is ahead of the base'
is "$(git rev-parse integration/rerun-check)" "$int1" 'run 2b: and still untouched'
unlike "$OUT" 'rerun-check/t2' 'run 2b: the branch it no longer objects to is not named'

## --------------------------------------------- the recovery the message names

git merge -q --ff-only integration/rerun-check
newbase=$(git rev-parse HEAD)
isnt "$newbase" "$base" "recovery: run 1's work is on the trunk now"

rm -f "$T_TMP/misbehave-t2"
run workflow run
is "$RC" 0 'run 3: the recovery path runs to completion'
is "$(cat "$rundir/t1.state")" merged \
	'run 3: the task ticked off in run 1 counts as merged, its commit being on integration'
is "$(cat "$rundir/t2.state")" merged 'run 3: the parked task ran again and merged'
is "$(cat "$rundir/t2.parked")" '' \
	'run 3: and the park reason from run 1 is cleared, not left to be reported forever'
is "$(cat "$rundir/t2.bundle")" '' 'run 3: as is the bundle path it no longer needs'

run git cat-file -e "integration/rerun-check:app/t1.php"
is "$RC" 0 "run 3: t1's work is present on integration"
run git cat-file -e "integration/rerun-check:app/t2.php"
is "$RC" 0 "run 3: and so is t2's"
is "$(git rev-list --count "$newbase..integration/rerun-check")" 1 \
	'run 3: integration is ahead of the base it started from'
is "$(git worktree list | grep -c .)" 1 'run 3: no worktrees left behind'

## --------------------------- a tick whose work is nowhere near the integration

new_repo pre
mem_register
php_fixture
cat >"$T_TMP/pre-ticked.md" <<'PLAN'
# plan: pre-ticked

- [x] t0 Something that was done by hand
      Files: app/t0.php
      Verify: true
- [ ] t1 The one to run
      Files: app/t1.php
      Verify: true
- [ ] t2 The one that waits for the tick [after: t0]
      Files: app/t2.php
      Verify: true
PLAN
run workflow run --plan-file "$T_TMP/pre-ticked.md"
is "$RC" 0 'pre-ticked: the run completes'
prerun="$XDG_STATE_HOME/workflow/runs/pre/pre-ticked"
is "$(cat "$prerun/t0.state")" done-previously \
	'pre-ticked: a tick this orchestrator never merged is not called merged'
like "$OUT" 'already ticked off' 'pre-ticked: and the run says so out loud'
is "$(cat "$prerun/t1.state")" merged 'pre-ticked: the ordinary task merged'
is "$(cat "$prerun/t2.state")" merged \
	'pre-ticked: a task waiting on a hand-done tick is not blocked by it'
