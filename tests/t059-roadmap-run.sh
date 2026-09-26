#!/usr/bin/env bash
# The roadmap at run time. A roadmap is milestones, not work a worker can be
# handed, so `workflow run` refuses one and says which verb turns a milestone
# into the plan of record. The other half is the tick: a milestone whose plan
# came out of mem and finished is checked off in the roadmap, and a plan that
# stopped short or came from a file is not.
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"

# The fake worker: one commit per task, except sulk, which reports blocked,
# and reopen, which first files the plan of record again with every box open --
# standing in for an orchestrator editing the plan while the run is going --
# through mem, which keeps the ticks (friction #6K4RFP7Q), and then behind
# mem's back, which only the run's end can put right.
write_exec "$T_TMP/fake-worker.sh" <<'FAKE'
#!/bin/sh
task=$1; status=$3
say() { printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" >>"$status"; }
say started
if [ "$task" = sulk ]; then
	say 'blocked I would rather not'
	printf '{"is_error":false,"result":"ok"}\n'
	exit 0
fi
if [ "$task" = reopen ]; then
	"$WORKFLOW_MEM" plan --set-file "$WF_TMP/reopened-milestone.md" >/dev/null 2>"$WF_TMP/reopen.mem"
	sed -i 's/^- \[x\] t1 /- [ ] t1 /' "$(find "$XDG_DATA_HOME/mem/store/projects" -name plan.md)"
fi
# Named for its plan as well as its task: once a finished milestone lands on
# the checkout, the next plan's t1 starts over the last one's app/t1.php, and a
# byte-identical write would leave nothing to commit.
plan=$(basename "$(dirname "$2")")
mkdir -p app
printf '%s %s\n' "$plan" "$task" >"app/$task.php"
git add "app/$task.php"
git -c core.hooksPath=/dev/null commit -qm "Add the $task service"
say ready
printf '{"is_error":false,"result":"ok"}\n'
FAKE

export FAKE="$T_TMP/fake-worker.sh"
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$FAKE" {task} {worktree} {status}'"'"' > {out} 2> {err} &'

new_repo app
mem_register
printf '{"name":"acme/app"}\n' >composer.json
printf '#!/bin/sh\nexit 0\n' >artisan
chmod +x artisan
write_exec bin/php <<-'EOF'
	#!/bin/sh
	exit 0
EOF
git add -A
git -c core.hooksPath=/dev/null commit -qm 'project files'

"$MEM_BIN" roadmap --stdin >/dev/null <<'EOF'
# roadmap: app-2026

- [ ] filed-milestone The one whose plan is read off a file
- [ ] copied-milestone The one run off a copy of the plan of record
- [ ] sulky-milestone The one whose worker will not
- [ ] done-milestone The one that finishes
- [ ] reopened-milestone The one whose plan is rewritten under it
EOF

# plan <slug> <task-two> -- a two-task plan in $T_TMP/<slug>.md.
plan() {
	cat >"$T_TMP/$1.md" <<-EOF
	# plan: $1

	- [ ] t1 Add the t1 service
	      Files: app/t1.php
	      Verify: true
	- [ ] $2 Add the $2 service
	      Files: app/$2.php
	      Verify: true
	EOF
}

## ------------------------------------------- a roadmap is not run, it is cut

cat >"$T_TMP/roadmap.md" <<'EOF'
# roadmap: app-2026

- [ ] filed-milestone The one whose plan is read off a file
- [ ] done-milestone The one that finishes
EOF
run workflow run --plan-file "$T_TMP/roadmap.md"
is "$RC" 2 'a roadmap handed to run is refused'
like "$OUT" "'app-2026' is a roadmap" 'named, so the reader knows which document'
like "$OUT" 'milestones rather than work a worker can take' 'and told why'
like "$OUT" 'mem plan --from <slug>' 'with the verb that turns one into the plan of record'
is "$(git worktree list | grep -c .)" 1 'and no worktree was made'
[ -d "$XDG_STATE_HOME/workflow/worktrees/app/app-2026" ] && notok 'nor a worker dispatched' 'the run set up' || ok 'nor a worker dispatched'

# The same document filed as the plan of record: mem stores what it is given,
# so this is the mistake to catch on the way in rather than at the first
# milestone dispatched as a task.
"$MEM_BIN" plan --set-file "$T_TMP/roadmap.md" >/dev/null
run workflow run
is "$RC" 2 'a roadmap filed as the plan of record is refused the same way'
like "$OUT" 'mem plan --from <slug>' 'with the same remedy'

## ------------------------------------------ a finished milestone is ticked

plan done-milestone t2
"$MEM_BIN" plan --set-file "$T_TMP/done-milestone.md" >/dev/null
run env WORKFLOW_DEADLINE_MIN=0.5 workflow run
is "$RC" 0 'the milestone plan runs to the end'
is "$(cat "$XDG_STATE_HOME/workflow/runs/app/done-milestone/t2.state")" merged 'with every task merged'
like "$OUT" 'milestone done-milestone is ticked off in the roadmap' 'and the run says what it ticked'
run_out "$MEM_BIN" roadmap
like "$OUT" '^- \[x\] done-milestone ' 'and its slug is ticked off in the roadmap'
like "$OUT" '^- \[ \] sulky-milestone ' 'leaving the milestones behind it alone'
# A run that merged everything lands its integration branch on the checkout's
# own branch, a fast-forward and nothing else, and says so; left on
# integration it sat for hours with nobody sure whose move it was (#EVSE2061).
run_out git rev-parse HEAD
is "$OUT" "$(git rev-parse integration/done-milestone)" 'and the checkout is fast-forwarded to integration'
like "$(cat "$XDG_STATE_HOME/workflow/runs/app/done-milestone/events")" 'landed integration/done-milestone on ' \
	'with the landing in the run log'

## ------------------------------------- a milestone that stopped short is not

plan sulky-milestone sulk
"$MEM_BIN" plan --set-file "$T_TMP/sulky-milestone.md" >/dev/null
run env WORKFLOW_DEADLINE_MIN=0.5 workflow run
is "$RC" 1 'a task that never reported ready stops the run short'
unlike "$OUT" 'ticked off in the roadmap' 'so the run claims no milestone'
run_out "$MEM_BIN" roadmap
like "$OUT" '^- \[ \] sulky-milestone ' 'and the milestone stays unticked'

## ------------------------------------------- and a plan read off a file is not

# A --plan-file plan need not be in mem at all: unless its slug is the plan of
# record's, nothing here can say which milestone it is, even when the names
# line up. The plan of record is sulky-milestone here, so this one is a stranger.
plan filed-milestone t2
run env WORKFLOW_DEADLINE_MIN=0.5 workflow run --plan-file "$T_TMP/filed-milestone.md"
is "$RC" 0 'a run off a plan file finishes the same way'
unlike "$OUT" 'ticked off in the roadmap' 'without a word about the roadmap'
like "$(cat "$T_TMP/filed-milestone.md")" '^- \[x\] t1 ' 'ticking its tasks off in the file it was handed'
run_out "$MEM_BIN" roadmap
like "$OUT" '^- \[ \] filed-milestone ' 'and leaving the roadmap alone'

## -------------------- but a copy of the plan of record ticks mem and the roadmap

# The restart shape: a run started again off `mem plan > file` (the workaround
# for a plan edited mid-run) used to tick its copy alone, so the milestone read
# as untouched in mem and the roadmap after nine merges (friction #YY1F6P20).
plan copied-milestone t2
"$MEM_BIN" plan --set-file "$T_TMP/copied-milestone.md" >/dev/null
cp "$T_TMP/copied-milestone.md" "$T_TMP/copied-milestone.copy.md"
run env WORKFLOW_DEADLINE_MIN=0.5 workflow run --plan-file "$T_TMP/copied-milestone.copy.md"
is "$RC" 0 'a run off a copy of the plan of record finishes'
like "$(cat "$T_TMP/copied-milestone.copy.md")" '^- \[x\] t2 ' 'ticking the file it was handed'
run_out "$MEM_BIN" plan
like "$OUT" '^- \[x\] t1 ' 'and the plan of record in mem'
like "$OUT" '^- \[x\] t2 ' 'every task of it'
run_out "$MEM_BIN" roadmap
like "$OUT" '^- \[x\] copied-milestone ' 'and the milestone in the roadmap'

## ------------------------- a plan rewritten under the run is ticked again

# The plan of record is a live document -- read fresh at every dispatch, and
# edited mid-run by whoever is answering questions. An edit that reopens a box
# this run already ticked used to stand, so a merged task read as work still to
# do and the next run built it again. `mem plan` keeps the ticks its copy has
# when the same document is filed again; a write that bypasses mem is caught
# at the end, where every merge is ticked once more against the plan as it
# reads then.
cat >"$T_TMP/reopened-milestone.md" <<'EOF'
# plan: reopened-milestone

- [ ] t1 Add the t1 service
      Files: app/t1.php
      Verify: true
- [ ] reopen Add the reopen service [after: t1]
      Files: app/reopen.php
      Verify: true
EOF
"$MEM_BIN" plan --set-file "$T_TMP/reopened-milestone.md" >/dev/null
run env WORKFLOW_DEADLINE_MIN=0.5 workflow run
is "$RC" 0 'the run finishes though the plan was rewritten under it'
like "$(cat "$WF_TMP/reopen.mem")" 'kept the tick on t1: the plan has them ticked' \
	'filing the plan through mem kept the tick the run had made'
like "$OUT" 'task t1: had come unticked in the plan of record -- ticked again' \
	'and the box unticked behind mem is named as ticked again'
like "$OUT" 'milestone reopened-milestone is ticked off in the roadmap' \
	'so the milestone is finished after all'
like "$("$MEM_BIN" log --limit 20 --json)" \
	'task t1: had come unticked in the plan of record -- ticked again' \
	'with the same line in the mem log'
run_out "$MEM_BIN" plan
like "$OUT" '^- \[x\] t1 ' 'the reopened box is ticked in the plan of record'
like "$OUT" '^- \[x\] reopen ' 'beside the task that rewrote it'
run_out "$MEM_BIN" roadmap
like "$OUT" '^- \[x\] reopened-milestone ' 'and the milestone is ticked in the roadmap'

## ----------------------------------------- a tick mem refuses is said out loud

# m6-commerce and m9-pages read their plan from mem, merged every task and
# never ticked the roadmap, with not a word said: a tick mem refused read the
# same as one with nothing to do (frictions #7KTQPJQK, #0W95EPF4). mem's own
# reason goes to the warning, the event and the run log, with the command
# that ticks it by hand.
plan stray-milestone t2
"$MEM_BIN" plan --set-file "$T_TMP/stray-milestone.md" >/dev/null
run env WORKFLOW_DEADLINE_MIN=0.5 workflow run
is "$RC" 0 'a plan the roadmap does not name still runs to the end'
like "$OUT" "roadmap tick for stray-milestone failed: .*no milestone 'stray-milestone' in the roadmap" \
	'the warning carries what mem said'
like "$OUT" 'fix: mem --project app roadmap --tick stray-milestone' 'and names the command that ticks it'
like "$(cat "$XDG_STATE_HOME/workflow/runs/app/stray-milestone/events")" \
	'roadmap tick for stray-milestone failed: .*fix: mem --project app roadmap --tick stray-milestone' \
	'with the same line in the run log'
# The log's title is cut short, so the fix command is only begun there.
like "$("$MEM_BIN" log --limit 20 --json)" \
	"run stray-milestone: roadmap tick failed: no milestone 'stray-milestone' in the roadmap; fix: mem" \
	'and in the mem log'

## ------------------------------------------- no roadmap at all is no failure

"$MEM_BIN" roadmap --clear >/dev/null
plan unmapped-milestone t2
"$MEM_BIN" plan --set-file "$T_TMP/unmapped-milestone.md" >/dev/null
run env WORKFLOW_DEADLINE_MIN=0.5 workflow run
is "$RC" 0 'a plan in a project with no roadmap runs to the end'
unlike "$OUT" 'roadmap' 'and says nothing about a roadmap it does not have'
