#!/usr/bin/env bash
# plan-check refuses two shapes a run cannot carry into a clean repo: a
# numbered list under a Rulings or Decisions heading, which workers then cite
# by number in code and commits, and a Verify line naming an absolute path,
# which points outside the worktree the task runs in. Both are refusals; the
# same plan with bullet decisions and relative paths passes.
source "$(dirname -- "$0")/lib.sh"
t_init

new_repo decisions
mem_register

plan() { cat >"plan.md"; }
check() {
	OUT=$(workflow plan-check plan.md 2>"$T_TMP/check.err")
	RC=$?
	ERR=$(cat "$T_TMP/check.err")
}

## ------------------------------------------------- numbered Rulings refuse

plan <<'EOF'
# plan: decisions

## Spec

Do the thing.

## Rulings

1. Prices are integers in cents, because floats drift.

- [ ] t1 Do the thing
      Files: a.rs
      Verify: true
EOF
check
is "$RC" 1 'a numbered Rulings list is refused'
like "$ERR" "plan: the Rulings heading holds a numbered list -- write decisions as sentences with their reason" \
	'the refusal names the heading and the remedy'

plan <<'EOF'
# plan: decisions

Do the thing.

### Decisions

2) Prices are integers in cents, because floats drift.

- [ ] t1 Do the thing
      Files: a.rs
      Verify: true
EOF
check
is "$RC" 1 'a numbered Decisions list is refused'
like "$ERR" "plan: the Decisions heading holds a numbered list -- write decisions as sentences with their reason" \
	'under any heading level and either list marker'

# A numbered list under another heading is the author's business.
plan <<'EOF'
# plan: decisions

## Spec

1. Read the cart.
2. Price it.

## Rulings

- Prices are integers in cents, because floats drift.

- [ ] t1 Do the thing
      Files: a.rs
      Verify: true
EOF
check
is "$RC" 0 'a numbered list under Spec is not refused'
unlike "$ERR" 'numbered list' 'and earns no finding'

## ------------------------------------------- an absolute path in Verify

plan <<'EOF'
# plan: decisions

## Rulings

- Prices are integers in cents, because floats drift.

- [ ] t1 Do the thing
      Files: a.rs
      Verify: cat /home/someone/checkout/a.rs && grep -q cents a.rs 2>/dev/null
EOF
check
is "$RC" 1 'a Verify naming an absolute path is refused'
like "$ERR" "plan: task t1: Verify names '/home/someone/checkout/a.rs' -- verify runs in a worktree: relative paths only" \
	'the refusal quotes the path and gives the remedy'
unlike "$ERR" "'/dev/null'" 'a redirection to /dev/null is no path into a checkout'

## ------------------------------------------------- bullet form passes

plan <<'EOF'
# plan: decisions

## Spec

Do the thing.

## Rulings

- Prices are integers in cents, because floats drift.

## Decisions

- The cart keeps its own total, so the page never recomputes it.

- [ ] t1 Do the thing
      Files: a.rs
      Verify: cat a.rs >/dev/null
EOF
check
is "$RC" 0 'a bullet-form plan with relative paths passes'
unlike "$ERR" 'numbered list' 'with no decisions finding'
unlike "$ERR" 'relative paths only' 'and no Verify finding'
