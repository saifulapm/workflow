#!/usr/bin/env bash
# A Rust project's builds are isolated per worktree. One shared
# CARGO_TARGET_DIR let a task's suite drive another task's binary, and left
# binaries whose baked-in paths pointed at reaped worktrees (frictions
# #MQRKM0AD, #TFVWXXDQ). Each worker now gets its own target dir under the
# run, the gate gets its own, and cleanup takes them down with the worktrees.
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"

write_exec "$T_TMP/fake-worker.sh" <<'FAKE'
#!/bin/sh
task=$1; status=$3
say() { printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" >>"$status"; }
say started
printf '%s\n' "${CARGO_TARGET_DIR:-unset}" >"$WF_TMP/$task-target"
mkdir -p src
printf '%s\n' "$task" >"src/$task.rs"
git add "src/$task.rs"
git -c core.hooksPath=/dev/null commit -qm "Add the $task module"
say ready
printf '{"is_error":false,"result":"ok"}\n'
FAKE

export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$WF_TMP/fake-worker.sh" {task} {worktree} {status} {session}'"'"' > {out} 2> {err} &'

new_repo cargotest
mem_register
# The gate runs the project verifier; a recorder instead of cargo keeps the
# fixture fast and shows which target dir the gate itself was given.
write_exec "$T_TMP/gate-verify.sh" <<'EOF'
#!/bin/sh
printf '%s\n' "${CARGO_TARGET_DIR:-unset}" >>"$WF_TMP/gate-target"
exit 0
EOF
"$MEM_BIN" project set verify "$T_TMP/gate-verify.sh" >/dev/null

printf '[package]\nname = "cargotest"\nversion = "0.1.0"\nedition = "2021"\n' >Cargo.toml
mkdir -p src
printf '\n' >src/lib.rs
git add -A
git -c core.hooksPath=/dev/null commit -qm 'project files'

"$MEM_BIN" plan --stdin >/dev/null <<'EOF'
# plan: cargo-iso

- [ ] t1 First module
      Files: src/t1.rs
      Verify: true
- [ ] t2 Second module
      Files: src/t2.rs
      Verify: true
EOF

# An inherited target dir must not become shared state between the workers.
export CARGO_TARGET_DIR="$T_TMP/outer-target"

export WORKFLOW_MAX_WORKERS=2 WORKFLOW_DEADLINE_MIN=0.5
run workflow run
is "$RC" 0 'the run completes'

cargo_root="$XDG_STATE_HOME/workflow/cargo/cargotest/cargo-iso"

t1=$(cat "$T_TMP/t1-target")
t2=$(cat "$T_TMP/t2-target")
is "$t1" "$cargo_root/t1" 'the first worker builds in its own target dir'
is "$t2" "$cargo_root/t2" 'the second worker builds in its own target dir'
isnt "$t1" "$T_TMP/outer-target" 'an inherited CARGO_TARGET_DIR does not leak in'

like "$(cat "$T_TMP/gate-target")" "cargo-iso/integration\$" \
	'the gate builds in a target dir of its own, not in any worker'"'"'s'

[ -d "$cargo_root" ] && root_state=present || root_state=absent
is "$root_state" absent 'cleanup takes the target dirs down with the worktrees'
