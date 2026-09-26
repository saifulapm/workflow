#!/usr/bin/env bash
# `workflow plan-check` on a data file: a Files entry ending in a data
# extension is looked up by basename across the tracked tree, and a tracked
# file naming it that the task's own Files does not claim warns -- the
# assertion three amx pi-screens dispatches paid to rediscover (#JRS7GAA5).
source "$(dirname -- "$0")/lib.sh"
t_init

new_repo data
mkdir -p assets tests
printf 'title = "demo"\n' >assets/rules.toml
printf '# self.toml is the source of truth\n' >assets/self.toml
printf 'grep rules.toml assets/rules.toml\n' >tests/rules_test.sh
git add assets/rules.toml assets/self.toml tests/rules_test.sh
git -c core.hooksPath=/dev/null commit -qm 'rules, a self-naming file and their test'

plan() { cat >"plan.md"; }
check() {
	OUT=$(workflow plan-check plan.md 2>"$T_TMP/check.err")
	RC=$?
	ERR=$(cat "$T_TMP/check.err")
}

## --------------------------------------- a data file named outside Files

plan <<'EOF'
# plan: assets

- [ ] t1 Load the screen rules
      Files: assets/rules.toml
      Verify: true
- [ ] t2 Load the self-describing file
      Files: assets/self.toml
      Verify: true
EOF

check
is "$RC" 0 'warnings do not refuse the plan'
like "$ERR" \
	"plan: task t1: assets/rules\.toml is named by tests/rules_test\.sh, which Files does not claim" \
	'a tracked file naming the basename outside Files warns'
unlike "$ERR" 'task t2:.*is named by' \
	'a basename only its own file mentions produces no warning'

## ------------------------------------------- the naming file joins Files

plan <<'EOF'
# plan: assets

- [ ] t1 Load the screen rules and their test
      Files: assets/rules.toml tests/rules_test.sh
      Verify: true
- [ ] t2 Load the self-describing file
      Files: assets/self.toml
      Verify: true
EOF

check
is "$RC" 0 'still not refused'
unlike "$ERR" 'is named by' \
	'claiming the naming file in Files clears the warning'

## --------------------------------------------- a tracked plan is not a hit

plan <<'EOF'
# plan: assets

- [ ] t1 Load the screen rules
      Files: assets/rules.toml
      Verify: true
- [ ] t2 Load the self-describing file
      Files: assets/self.toml
      Verify: true
EOF
git add plan.md
git -c core.hooksPath=/dev/null commit -qm 'track the plan too'

check
is "$RC" 0 'warnings do not refuse the plan'
like "$ERR" \
	"plan: task t1: assets/rules\.toml is named by tests/rules_test\.sh, which Files does not claim" \
	'the real hit still warns once the plan itself is tracked'
unlike "$ERR" 'named by plan\.md' \
	"the plan's own Files line mentioning a basename is not counted as a hit on itself"

## ------------------------------- a non-ascii naming file is read, not quoted

new_repo data2
mkdir -p assets tests
printf 'title = "demo"\n' >assets/rules.toml
printf 'grep rules.toml assets/rules.toml\n' >'tests/café_test.sh'
git add assets/rules.toml 'tests/café_test.sh'
git -c core.hooksPath=/dev/null commit -qm 'rules and a non-ascii test naming it'

plan <<'EOF'
# plan: assets

- [ ] t1 Load the screen rules and their test
      Files: assets/rules.toml tests/café_test.sh
      Verify: true
EOF

check
is "$RC" 0 'warnings do not refuse the plan'
unlike "$ERR" 'is named by' \
	'a non-ascii naming file claimed in Files is matched, quoting and all'

## ------------------------------------------- a glob Files entry is expanded

new_repo data3
mkdir -p assets tests
printf 'title = "demo"\n' >assets/rules.toml
printf '# self.toml is the source of truth\n' >assets/self.toml
printf 'grep rules.toml assets/rules.toml\n' >tests/rules_test.sh
git add assets/rules.toml assets/self.toml tests/rules_test.sh
git -c core.hooksPath=/dev/null commit -qm 'rules, a self-naming file and their test'

plan <<'EOF'
# plan: assets

- [ ] t1 Load every screen rule
      Files: assets/*.toml
      Verify: true
EOF

check
is "$RC" 0 'warnings do not refuse the plan'
like "$ERR" \
	"plan: task t1: assets/rules\.toml is named by tests/rules_test\.sh, which Files does not claim" \
	'a glob Files entry is expanded to the tracked files it matches'
unlike "$ERR" 'assets/self\.toml is named by' \
	'a basename only its own file mentions produces no warning under a glob either'

## ------------------------- a non-data Files entry is not expanded for data

plan <<'EOF'
# plan: assets

- [ ] t1 Load the whole assets tree
      Files: assets/
      Verify: true
EOF

check
is "$RC" 0 'warnings do not refuse the plan'
unlike "$ERR" 'is named by' \
	"a directory Files entry is not gated as data even though ls-files expands it to a .toml path"

## ---------------------------- overlapping Files entries warn once each

plan <<'EOF'
# plan: assets

- [ ] t1 Load every screen rule, named twice
      Files: assets/*.toml assets/rules.toml
      Verify: true
EOF

check
is "$RC" 0 'warnings do not refuse the plan'
COUNT=$(grep -Ec \
	"plan: task t1: assets/rules\.toml is named by tests/rules_test\.sh, which Files does not claim" \
	<<<"$ERR")
is "$COUNT" 1 'a file two Files entries both cover is named once, not once per entry'

## ------------------------------------- a Verify grepping outside Files

# A Verify that greps a file the task does not own asks the worker to edit
# it, and the commit hook refuses the edit (friction #G1JWHABM).
plan <<'EOF'
# plan: assets

- [ ] t1 Retitle the rules
      Files: assets/self.toml
      Verify: grep -q 'title = "demo"' assets/rules.toml && rg -e demo -e 'a|b' assets/self.toml | grep -c demo
EOF

check
is "$RC" 0 'warnings do not refuse the plan'
like "$ERR" \
	"plan: task t1: Verify greps 'assets/rules\.toml' and no Files: pattern claims it" \
	'a grep operand outside Files warns'
unlike "$ERR" "Verify greps 'assets/self\.toml'" 'an rg operand Files claims does not'
unlike "$ERR" "Verify greps '(title|demo|a\|b)" 'the search pattern is not an operand'

# A word in operand position that is no tracked file is not a path, and a
# leading `cd <dir> &&` moves the operand under that dir.
plan <<'EOF'
# plan: assets

- [ ] t1 Retitle the rules
      Files: assets/self.toml
      Verify: grep -A 3 'fn main' assets/self.toml 2>/dev/null && grep -q x "$OUT_FILE" && grep -rn x . && cd assets && grep -q demo self.toml
EOF

check
unlike "$ERR" "Verify greps" 'patterns, redirections, variables, dirs and a cd-relative owned file do not warn'
