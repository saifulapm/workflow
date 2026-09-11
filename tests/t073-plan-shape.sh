#!/usr/bin/env bash
# plan-check's shape warnings (ruling 3 of m1-wiki-first): a plan with no
# prose above its tasks, a task whose Files carries more than eight
# patterns, a Done sentence over forty words, and a Read: naming a wiki page
# the project has no such page -- four warnings, never a refusal. A wiki:
# item never trips the existing "not here to be read" warning, and a
# roadmap's milestone plan is judged on its spec through wiki pages, not on
# carrying its own prose (ruling 10).
source "$(dirname -- "$0")/lib.sh"
t_init

new_repo shape
mem_register

plan() { cat >"plan.md"; }
check() {
	OUT=$(workflow plan-check plan.md 2>"$T_TMP/check.err")
	RC=$?
	ERR=$(cat "$T_TMP/check.err")
}

## ------------------------------------------------------- no prose at all

plan <<'EOF'
# plan: shape

- [ ] t1 Do the thing
      Files: a.rs
      Verify: true
EOF
check
is "$RC" 0 'warnings do not refuse the plan'
like "$ERR" \
	"plan: no prose above the tasks -- the worker and the reader see only the task blocks; write the Spec and the Rulings first" \
	'a plan with nothing above its tasks warns'

## ----------------------------------------------------------- with prose

plan <<'EOF'
# plan: shape

## Spec

Do the thing.

## Rulings

1. Do it well.

- [ ] t1 Do the thing
      Files: a.rs
      Verify: true
EOF
check
unlike "$ERR" 'no prose above the tasks' 'prose above the tasks clears the warning'

## ------------------------------------------------------------- Files width

plan <<'EOF'
# plan: shape

## Spec

Do the thing.

## Rulings

1. Do it well.

- [ ] t1 Do the thing
      Files: a.rs b.rs c.rs d.rs e.rs f.rs g.rs h.rs i.rs
      Verify: true
EOF
check
is "$RC" 0 'warnings do not refuse the plan'
like "$ERR" "plan: task t1: Files carries 9 patterns -- a task owning more than eight is two tasks" \
	'a task with nine Files patterns warns'

plan <<'EOF'
# plan: shape

## Spec

Do the thing.

## Rulings

1. Do it well.

- [ ] t1 Do the thing
      Files: a.rs b.rs c.rs d.rs e.rs f.rs g.rs h.rs
      Verify: true
EOF
check
unlike "$ERR" 'Files carries' 'eight patterns is still one task'

## -------------------------------------------------------------- Done length

done_long=$(printf 'word %.0s' $(seq 1 41))
plan <<EOF
# plan: shape

## Spec

Do the thing.

## Rulings

1. Do it well.

- [ ] t1 Do the thing
      Files: a.rs
      Verify: true
      Done: $done_long
EOF
check
like "$ERR" "plan: task t1: Done is 41 words -- a sentence over forty is a fix verdict waiting; split the task or the sentence" \
	'a Done sentence over forty words warns'

done_ok=$(printf 'word %.0s' $(seq 1 40))
plan <<EOF
# plan: shape

## Spec

Do the thing.

## Rulings

1. Do it well.

- [ ] t1 Do the thing
      Files: a.rs
      Verify: true
      Done: $done_ok
EOF
check
unlike "$ERR" 'Done is' 'forty words is still one sentence'

## ------------------------------------------------------------ wiki pages

printf 'The run drives waves.\n' | "$MEM_BIN" wiki run --stdin --note seed >/dev/null

plan <<'EOF'
# plan: shape

## Spec

Do the thing.

## Rulings

1. Do it well.

- [ ] t1 Do the thing
      Files: a.rs
      Read: wiki:run wiki:missing
      Verify: true
EOF
check
is "$RC" 0 'warnings do not refuse the plan'
like "$ERR" "plan: task t1: Read names wiki:missing and the project has no such page" \
	'a wiki: page the project does not have warns'
unlike "$ERR" 'wiki:run and the project has no such page' \
	'a wiki: page the project does have does not warn'
unlike "$ERR" "Read names 'wiki:run' and it is not here to be read" \
	'a wiki: item never trips the "not here to be read" warning'
unlike "$ERR" "Read names 'wiki:missing' and it is not here to be read" \
	'not even an absent one'

## ------------------------------------------ a milestone plan in a roadmap

# A roadmap's milestone plan is judged on its spec through wiki pages, so a
# milestone with no prose of its own does not earn the "no prose" warning --
# only a plan checked on its own does.
mkdir -p road
cat >road/roadmap.md <<'EOF'
# roadmap: shop

- [ ] m1 First
EOF
cat >road/m1.md <<'EOF'
# plan: m1

- [ ] t1 Do the thing
      Files: a.rs
      Verify: true
EOF
OUT=$(workflow plan-check road/roadmap.md 2>"$T_TMP/road.err")
RC=$?
ROAD_ERR=$(cat "$T_TMP/road.err")
is "$RC" 0 'the roadmap is not refused'
unlike "$ROAD_ERR" 'no prose above the tasks' \
	"a milestone plan's own lack of prose is not warned about"
