#!/usr/bin/env bash
# plan-check warns on a block that points at another task -- "as in t1",
# "see t2", "t1's helper" -- since a worker sees only its own block and the
# plan's prose, never a sibling's; and on a Uses that a task of the same
# plan Gives with no [after:] between the two, named with the edge that
# mends it rather than as a name nobody gives. Warnings both, never a
# refusal.
source "$(dirname -- "$0")/lib.sh"
t_init

new_repo pointers
mem_register

plan() { cat >"plan.md"; }
check() {
	OUT=$(workflow plan-check plan.md 2>"$T_TMP/check.err")
	RC=$?
	ERR=$(cat "$T_TMP/check.err")
}

## -------------------------------------------- a block that points at a task

plan <<'EOF2'
# plan: pointers

## Spec

Do the thing.

## Rulings

1. Do it well.

- [ ] t1 Give the ledger
      Files: src/ledger.rs
      Gives: Ledger::append(&mut self, kind: EntryKind) -> EntryId
      Verify: true
- [ ] t2 Read the ledger like t1  [after: t1]
      Files: src/reader.rs
      Uses: t1's Ledger::append
      Verify: true
      Done: every entry is read back, same as t1
EOF2
check
is "$RC" 0 'a pointer warns, it does not refuse'
like "$ERR" "plan: task t2: the title says 'like t1' -- a worker sees only its own block; say here what t1 does or gives, spelled as t1 spells it" \
	'a title pointing at a sibling is named with the phrase'
like "$ERR" "plan: task t2: Uses says 't1's'" 'a possessive on an id is a pointer'
like "$ERR" "plan: task t2: Done says 'same as t1'" 'and so is a Done that defers to a sibling'
unlike "$ERR" 'plan: task t1: .*sees only its own block' 'the task pointed at has nothing to answer for'

## ------------------------------------------- an id that is a word is prose

plan <<'EOF2'
# plan: pointers

## Spec

Do the thing.

## Rulings

1. Do it well.

- [ ] brief Write the brief
      Files: src/brief.rs
      Verify: true
- [ ] reader Read the brief in brief  [after: brief]
      Files: src/reader.rs
      Verify: true
      Done: the reader holds the brief to the plan, unlike brief
EOF2
check
is "$RC" 0 'a bare mention is fine'
unlike "$ERR" 'sees only its own block' 'an id that is also a word, mentioned bare or after another word, is prose'

## ------------------------ a Uses a task of this plan Gives with no edge

plan <<'EOF2'
# plan: pointers

## Spec

Do the thing.

## Rulings

1. Do it well.

- [ ] t1 Use the ledger
      Files: src/reader.rs
      Uses: Ledger::append(&mut self, kind: EntryKind) -> EntryId
      Verify: true
- [ ] t2 Give the ledger
      Files: src/ledger.rs
      Gives: Ledger::append(&mut self, kind: EntryKind) -> EntryId
      Verify: true
EOF2
check
is "$RC" 0 'an unordered pair warns, it does not refuse'
like "$ERR" "plan: task t1: Uses 'Ledger::append\(&mut self, kind: EntryKind\) -> EntryId' and t2 Gives it, but t1 does not wait on t2 -- the two may run at once; add \[after: t2\] or order them the other way" \
	'the giver and the missing edge are named'
unlike "$ERR" 'no task it waits for Gives it, nor does the tree' 'and the symbol is not also reported as ungiven'

plan <<'EOF2'
# plan: pointers

## Spec

Do the thing.

## Rulings

1. Do it well.

- [ ] t1 Give the ledger
      Files: src/ledger.rs
      Gives: Ledger::append(&mut self, kind: EntryKind) -> EntryId
      Verify: true
- [ ] t2 Use the ledger  [after: t1]
      Files: src/reader.rs
      Uses: Ledger::append(&mut self, kind: EntryKind) -> EntryId
      Verify: true
EOF2
check
unlike "$ERR" 'may run at once' 'an ordered pair is no finding'
