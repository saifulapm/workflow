#!/usr/bin/env bash
# workflow run claims the project for this machine while it runs and clears the
# claim when it ends; another machine's live claim refuses the run, a stale one
# is taken over.
source "$(dirname -- "$0")/lib.sh"
t_init

mkdir -p "$XDG_CONFIG_HOME/qshell"
printf 'here\n' >"$XDG_CONFIG_HOME/qshell/machine"

new_repo claimed
mem_register
"$MEM_BIN" project set verify true >/dev/null
name=$("$MEM_BIN" project current --json | sed -n 's/.*"name":"\([^"]*\)".*/\1/p')
key() { "$MEM_BIN" project current --json | sed -n "s/.*\"$1\":\"\\([^\"]*\\)\".*/\\1/p"; }

# Each worker writes down whose claim the project carries while it works.
write_exec "$T_TMP/claim-worker.sh" <<'FAKE'
#!/bin/sh
task=$1; status=$3
"$MEM_BIN" --project "$NAME" project current --json >"$WF_TMP/seen-$task.json"
mkdir -p src
printf '<?php\n' >"src/$task.php"
git add "src/$task.php"
git -c core.hooksPath=/dev/null commit -qm "Add a service"
printf '%s ready merge-ready\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$status"
printf '{"is_error":false,"result":"ok"}\n'
FAKE
export FAKE="$T_TMP/claim-worker.sh" WF_TMP="$T_TMP" NAME="$name" MEM_BIN
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$FAKE" {task} {worktree} {status} {session}'"'"' > {out} 2> {err} &'

cat >"$T_TMP/claim.md" <<'PLAN'
# plan: claim

- [ ] t1 Add the t1 service
      Files: src/t1.php
      Verify: true
- [ ] t2 Add the t2 service
      Files: src/t2.php
      Verify: true
PLAN

## ------------------------------------- a fresh claim from another machine

"$MEM_BIN" project set runner elsewhere >/dev/null
since=$(key runner_since)
run workflow run --plan-file "$T_TMP/claim.md"
is "$RC" 2 'a claim another machine took minutes ago refuses the run'
like "$OUT" "$name is run by elsewhere since $since" 'and says whose claim and since when'
truthy "$([ ! -e "$T_TMP/seen-t1.json" ] && echo 0 || echo 1)" 'nothing was dispatched'
is "$(key runner)" elsewhere 'the other machine keeps its claim'

## ------------------------------------------ a claim two hours old is taken

toml=$("$MEM_BIN" project set runner elsewhere --json | sed -n 's/.*"path":"\([^"]*\)".*/\1/p')
old=$(date -u -d '2 hours ago' +%Y-%m-%dT%H:%M:%SZ)
sed -i "s/^runner_since = .*/runner_since = \"$old\"/" "$toml"
is "$(key runner_since)" "$old" 'the claim reads two hours old'

run workflow run --plan-file "$T_TMP/claim.md"
is "$RC" 0 'a stale claim does not hold the run back'
like "$(cat "$T_TMP/seen-t1.json")" '"runner":"here"' 'while it ran the project was claimed by this machine'
like "$(cat "$T_TMP/seen-t2.json")" '"runner":"here"' 'for every task'
is "$(key runner)" '' 'and the claim is gone once the run ends'
is "$(key runner_since)" '' 'with its start time'

## ------------------------- a stale claim beside a run logged minutes ago

# The run above logged its start and end under an hour ago, so whoever holds
# the claim is plausibly still at it whatever its age says.
"$MEM_BIN" project set runner elsewhere >/dev/null
sed -i "s/^runner_since = .*/runner_since = \"$old\"/" "$toml"
cat >"$T_TMP/again.md" <<'PLAN'
# plan: again

- [ ] t3 Add the t3 service
      Files: src/t3.php
      Verify: true
- [ ] t4 Add the t4 service
      Files: src/t4.php
      Verify: true
PLAN
run workflow run --plan-file "$T_TMP/again.md"
is "$RC" 2 'a run logged under an hour ago keeps an old claim live'
like "$OUT" "$name is run by elsewhere since $old" 'and the refusal names the claim'

age_mem_items
run workflow run --plan-file "$T_TMP/again.md"
is "$RC" 0 'once the run log is old as well, the claim is taken'
is "$(key runner)" '' 'and cleared at the end'
