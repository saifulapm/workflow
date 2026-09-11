#!/usr/bin/env bash
# ruling 4: a --plan-file inside the checkout is refused -- its ticks would
# land in the tree. The suite's own plan files sit in $T_TMP, beside the
# sandbox repos, never inside one.
source "$(dirname -- "$0")/lib.sh"
t_init

new_repo app
mem_register

cat >plan-in-tree.md <<'PLAN'
# plan: in-tree

- [ ] t1 Just do this one thing
      Files: src/**
      Verify: true
PLAN

run workflow run --plan-file plan-in-tree.md
is "$RC" 2 'a plan file under the sandbox repo is refused'
like "$OUT" 'plan-in-tree\.md is inside the checkout' 'and names the file and why'
like "$OUT" 'mem plan <slug> --set-file' 'and says how to store it instead'
like "$OUT" 'run from mem' 'and how to run it'

cp plan-in-tree.md "$T_TMP/plan-beside.md"
run workflow run --plan-file "$T_TMP/plan-beside.md"
is "$RC" 0 'the same file, copied beside the checkout, runs'
like "$OUT" 'one task' 'and reads as a real plan, not a refusal'
