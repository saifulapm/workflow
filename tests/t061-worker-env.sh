#!/usr/bin/env bash
# Dispatch.env is what the orchestrator worked out for one task -- its own
# WORKFLOW_TASK, its own CARGO_TARGET_DIR. This checks it lands in the process
# the worker actually starts as, one task's values never bleeding into the
# other's.
source "$(dirname -- "$0")/lib.sh"
t_init

# A nested self-check (see the bottom of this file): report what the sandbox
# saw and stop, so the recursive run.sh invocation stays cheap instead of
# repeating the dispatch scenario below. This has to run before WF_TMP below
# is reassigned to this process's own T_TMP, so it still sees the value the
# outer invocation exported. It also has to fail closed on the token: gating
# on the bare presence of WF_T061_NESTED would let any stray copy of that
# name already in the environment -- a nested-of-nested run, say -- exit here
# having run zero checks, and the suite would report the whole file green.
if [ -n "${WF_T061_NESTED:-}" ] && [ "$WF_T061_NESTED" = "$(cat "${WF_TMP:-/nonexistent}/nested-token" 2>/dev/null)" ]; then
	printf '# WORKFLOW_TASK seen as %s\n' "${WORKFLOW_TASK:-unset}"
	printf '# CARGO_TARGET_DIR seen as %s\n' "${CARGO_TARGET_DIR:-unset}"
	exit 0
fi

export WF_TMP="$T_TMP"

write_exec "$T_TMP/fake-worker.sh" <<'FAKE'
#!/bin/sh
task=$1; status=$3
say() { printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" >>"$status"; }
say started
env >"$WF_TMP/$task-env"
mkdir -p src
printf '%s\n' "$task" >"src/$task.rs"
git add "src/$task.rs"
git -c core.hooksPath=/dev/null commit -qm "Add the $task module"
say ready
printf '{"is_error":false,"result":"ok"}\n'
FAKE

export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$WF_TMP/fake-worker.sh" {task} {worktree} {status} {session}'"'"' > {out} 2> {err} &'

new_repo envtest
mem_register

write_exec "$T_TMP/gate-verify.sh" <<'EOF'
#!/bin/sh
printf '%s\n' "${CARGO_TARGET_DIR:-unset}" >"$WF_TMP/gate-target"
exit 0
EOF
"$MEM_BIN" project set verify "$T_TMP/gate-verify.sh" >/dev/null

# No Cargo.toml anywhere in this repo: the per-builder dir is not conditioned
# on one being found.
mkdir -p src
printf '\n' >src/.keep
git add -A
git -c core.hooksPath=/dev/null commit -qm 'project files'

"$MEM_BIN" plan --stdin >/dev/null <<'EOF'
# plan: env-carry

- [ ] t1 First module
      Files: src/t1.rs
      Verify: true
- [ ] t2 Second module
      Files: src/t2.rs
      Verify: true
EOF

export WORKFLOW_MAX_WORKERS=2 WORKFLOW_DEADLINE_MIN=0.5
run workflow run
is "$RC" 0 'the run completes'

t1_env=$(cat "$T_TMP/t1-env")
t2_env=$(cat "$T_TMP/t2-env")

like "$t1_env" 'WORKFLOW_TASK=env-carry/t1$' "t1's worker names its own task"
like "$t2_env" 'WORKFLOW_TASK=env-carry/t2$' "t2's worker names its own task"
unlike "$t1_env" 'WORKFLOW_TASK=env-carry/t2$' "t1's worker does not see t2's task"

t1_target=$(grep '^CARGO_TARGET_DIR=' <<<"$t1_env")
t2_target=$(grep '^CARGO_TARGET_DIR=' <<<"$t2_env")
isnt "$t1_target" "" 'each worker gets a cargo target dir'
isnt "$t1_target" "$t2_target" 'each worker gets its own cargo target dir'
isnt "$(cat "$T_TMP/gate-target")" 'unset' 'the gate gets a cargo target dir too'

# tests/run.sh must not let an outer WORKFLOW_TASK or CARGO_TARGET_DIR
# through: a session running under the workflow has both set for its own
# task, and the suite it runs must neither misdirect its own build with them
# nor hand them down into a test's sandbox.
token="nested-$$-$RANDOM-$RANDOM"
printf '%s' "$token" >"$T_TMP/nested-token"

export WORKFLOW_TASK=junk-task CARGO_TARGET_DIR="$T_TMP/outer-junk-target"
run env -u MEM_BIN -u WORKFLOW_BIN WF_T061_NESTED="$token" bash "$WF_ROOT/tests/run.sh" worker-env
is "$RC" 0 'run.sh completes with a polluted WORKFLOW_TASK and CARGO_TARGET_DIR'

if [ -x "$WF_ROOT/mem/target/release/mem" ] && [ -x "$WF_ROOT/workflow/target/release/workflow" ]; then
	built=landed
else
	built=missing
fi
is "$built" landed 'the build landed at the paths run.sh expects'

# The check above alone proves nothing here: the outer `run.sh worker-env`
# invocation running this file already built both binaries at those paths
# before this test started. What proves run.sh pinned its own build dir
# rather than trusting the inherited one is that the junk dir never got
# written to.
is "$(test -e "$T_TMP/outer-junk-target" && echo used || echo untouched)" \
	untouched 'run.sh built into its own tree, not the inherited dir'

like "$OUT" '# WORKFLOW_TASK seen as unset' 'the sandbox does not see the outer WORKFLOW_TASK'
like "$OUT" '# CARGO_TARGET_DIR seen as unset' 'the sandbox does not see the outer CARGO_TARGET_DIR'
