#!/usr/bin/env bash
# A branch whose commits do not replay onto integration but whose tip merges
# whole lands as one commit. The worker had redone its change on
# integration's file -- merged integration in and resolved it -- and the
# replay of its first commit met the hunk its tip already agreed on, so the
# task failed twice with nothing to act on (friction #JZN4M4JE).
source "$(dirname -- "$0")/lib.sh"
t_init

new_repo app
mem_register
"$MEM_BIN" project set verify true >/dev/null
printf 'base\n' >shared.txt
git add shared.txt
git -c core.hooksPath=/dev/null commit -qm 'Add the shared file'
base=$(git rev-parse HEAD)

# t1 writes the shared file. t2 starts from the run's base instead of the
# integration tip it was cut from, writes its own version first, then merges
# integration in and resolves the conflict to both lines.
write_exec "$T_TMP/worker.sh" <<FAKE
#!/bin/sh
task=\$1; status=\$3
say() { printf '%s %s\n' "\$(date -u +%Y-%m-%dT%H:%M:%SZ)" "\$1" >>"\$status"; }
say started
if [ "\$task" = t1 ]; then
	printf 't1\n' >shared.txt
	git add shared.txt
	git -c core.hooksPath=/dev/null commit -qm 'Write the first line of the shared file'
else
	tip=\$(git rev-parse HEAD)
	git reset -q --hard $base
	printf 't2 early\n' >shared.txt
	git add shared.txt
	git -c core.hooksPath=/dev/null commit -qm 'Write the second line of the shared file'
	git -c core.hooksPath=/dev/null merge -q "\$tip" >/dev/null 2>&1
	printf 't1\nt2\n' >shared.txt
	git add shared.txt
	git -c core.hooksPath=/dev/null commit -qm 'Keep both lines of the shared file'
fi
say ready
printf '{"is_error":false,"result":"ok"}\n'
FAKE
export FAKE="$T_TMP/worker.sh"
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$FAKE" {task} {worktree} {status}'"'"' > {out} 2> {err} &'
export WORKFLOW_DEADLINE_MIN=0.5

"$MEM_BIN" plan --stdin >/dev/null <<'PLAN'
# plan: squash

- [ ] t1 Say t1 in the shared file
      Files: shared.txt
      Verify: true
- [ ] t2 Say t2 in the shared file [after: t1]
      Files: shared.txt
      Verify: true
PLAN

rundir="$XDG_STATE_HOME/workflow/runs/app/squash"
run workflow run
is "$RC" 0 'the run merges both'
is "$(cat "$rundir/t2.state")" merged 't2 is merged'
like "$OUT" 'task t2: its commits do not replay onto integration/squash, but the branch merges whole -- landing it as one commit' \
	'and says it landed the branch whole'
is "$(git show integration/squash:shared.txt)" "$(printf 't1\nt2')" 'with the resolved file'
is "$(git log -1 --format=%s integration/squash)" 'Keep both lines of the shared file' "under the branch tip's message"
