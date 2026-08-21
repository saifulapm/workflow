#!/usr/bin/env bash
# Two verifies of one project never run their suites at the same time
# (friction #VTB9VB1S). The false red at the gate paired `verify --gate` on
# integration with a worker's evidence run in its worktree; both execute
# through cmd_verify, so a per-project flock serializes them.
source "$(dirname -- "$0")/lib.sh"
t_init

new_repo app
mem_register

# The suite records when it enters and leaves. Overlap -- the false-red
# condition -- would read enter,enter.
write_exec "$T_TMP/suite.sh" <<'EOF'
#!/bin/sh
printf 'enter\n' >>"$LOG"
sleep 1
printf 'leave\n' >>"$LOG"
EOF
export LOG="$T_TMP/suite.log"
"$MEM_BIN" project set verify "$T_TMP/suite.sh" >/dev/null

workflow verify >/dev/null 2>&1 &
one=$!
workflow verify >/dev/null 2>&1 &
two=$!
wait "$one" "$two"

seq=$(tr '\n' ' ' <"$LOG")
is "$seq" 'enter leave enter leave ' 'two concurrent verifies serialize their suites'

# A second project locks its own file, not this one's.
new_repo other
printf 'touch %s/other-ran\n' "$T_TMP" >"$T_TMP/other-suite.sh"
chmod +x "$T_TMP/other-suite.sh"
"$MEM_BIN" project set verify "$T_TMP/other-suite.sh" >/dev/null
run workflow verify
is "$RC" 0 'a different project verifies green'
truthy "$([ -f "$T_TMP/other-ran" ] && echo 0 || echo 1)" 'and its suite really ran'
is "$(ls "$XDG_STATE_HOME/workflow/suite-lock" | wc -l)" 2 'each project holds a lock file of its own'
