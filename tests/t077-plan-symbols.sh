#!/usr/bin/env bash
# plan-check reads Uses against Gives item for item, asks that every type a
# signature names be defined somewhere, and holds the prose to the plan
# skill's budget (friction #J0RQN6WY: the eco roadmap passed a green check
# with fourteen Uses spelled unlike their Gives and ten types no task
# defined, all found by hand in review). Three warnings, never a refusal.
source "$(dirname -- "$0")/lib.sh"
t_init

new_repo symbols
mem_register

plan() { cat >"plan.md"; }
check() {
	OUT=$(workflow plan-check plan.md 2>"$T_TMP/check.err")
	RC=$?
	ERR=$(cat "$T_TMP/check.err")
}

## ------------------------------------------ a Uses spelled as its Gives is

plan <<'EOF'
# plan: symbols

## Spec

Do the thing.

## Rulings

1. Do it well.

- [ ] t1 Give the ledger
      Files: src/ledger.rs
      Gives: Ledger::append(&mut self, kind: EntryKind) -> Result<EntryId, LedgerError> · EntryKind::{User, Assistant} · EntryId(u64)
      Verify: true
- [ ] t2 Use the ledger  [after: t1]
      Files: src/loop.rs
      Uses: Ledger::append(&mut self, kind: EntryKind) -> Result<EntryId, LedgerError>
      Verify: true
EOF
check
is "$RC" 0 'an exact spelling is no finding'
unlike "$ERR" 'Uses names' 'a Uses spelled as its Gives is grounded'
unlike "$ERR" 'Gives it as' 'and there is no drift to report'
unlike "$ERR" 'names type' 'every type the signatures name is defined by an item'

## --------------------------------------- the same symbol, spelled another way

plan <<'EOF'
# plan: symbols

## Spec

Do the thing.

## Rulings

1. Do it well.

- [ ] t1 Give the ledger
      Files: src/ledger.rs
      Gives: Ledger::append(&mut self, kind: EntryKind) -> Result<EntryId, LedgerError> · EntryKind::{User, Assistant} · EntryId(u64)
      Verify: true
- [ ] t2 Use the ledger  [after: t1]
      Files: src/loop.rs
      Uses: Ledger::append(&mut self, kind: EntryKind) -> EntryId · EntryKind::User
      Verify: true
EOF
check
is "$RC" 0 'drift warns, it does not refuse'
like "$ERR" "plan: task t2: Uses 'Ledger::append\(&mut self, kind: EntryKind\) -> EntryId' and t1 Gives it as 'Ledger::append\(&mut self, kind: EntryKind\) -> Result<EntryId, LedgerError>' -- one spelling in both, or the worker hunts" \
	'a signature that grew a return type is named with both spellings'
like "$ERR" "plan: task t2: Uses 'EntryKind::User' and t1 Gives it as 'EntryKind::\{User, Assistant\}'" \
	'a variant out of an enum given whole is drift too'
unlike "$ERR" 'Uses names' 'drift is not reported a second time as an ungrounded name'

## ------------------------------------------- a type nothing defines

plan <<'EOF'
# plan: symbols

## Spec

Do the thing.

## Rulings

1. Do it well.

- [ ] t1 Give the loop
      Files: src/loop.rs
      Gives: Session::prompt(&mut self, text: &str) -> Result<Outcome, SessionError> · Outcome { stop: Stop, receipt: Receipt } · Stop::{EndTurn, Budget(BudgetKind)} · BudgetKind::{Task, Day}
      Verify: true
EOF
check
is "$RC" 0 'an undefined type warns, it does not refuse'
like "$ERR" "plan: task t1: Gives names type 'Receipt' inside 'Outcome \{ stop: Stop, receipt: Receipt \}' and nothing defines it" \
	'a field type no item defines is named'
unlike "$ERR" "names type 'Stop'" 'a type an item of the same plan defines is not'
unlike "$ERR" "names type 'BudgetKind'" 'nor one a variant carries when its own item defines it'
unlike "$ERR" "names type 'Outcome'" 'nor the type an item itself defines'
unlike "$ERR" "names type 'Session'" 'nor the type a constructor stands for'
unlike "$ERR" "names type 'EndTurn'" 'a variant is a definition, not a use'
unlike "$ERR" "names type 'SessionError'" 'an error type is its module'"'"'s own'
unlike "$ERR" "names type 'Result'" 'a library type is nobody'"'"'s to define'

## --------------------------------- prose inside an item is not a type

plan <<'EOF'
# plan: symbols

## Spec

Do the thing.

## Rulings

1. Do it well.

- [ ] t1 Give the perms
      Files: src/perms.rs
      Gives: Perms::reject_cascade(&mut self, reason: &str) -> String (the steering line the dispatcher appends as a User entry) · tokio::sync::broadcast::Receiver<Note> · Note { text: String }
      Verify: true
EOF
check
unlike "$ERR" "names type 'User'" 'a capitalised word after an article is prose'
unlike "$ERR" "names type 'Receiver'" 'a name reached through a lowercase path says where it lives'
unlike "$ERR" 'names type' 'and nothing else in the line is undefined'

## ------------------------------------------------ the prose budget

long=$(printf 'word %.0s' $(seq 1 1600))
plan <<EOF
# plan: symbols

## Spec

$long

## Rulings

1. Do it well.

- [ ] t1 Do the thing
      Files: a.rs
      Verify: true
EOF
check
is "$RC" 0 'a long prose warns, it does not refuse'
like "$ERR" 'plan: the prose is about [0-9]+ tokens \(bytes ÷ 4\) -- past the 1500 the plan skill sets for what every worker reads' \
	'prose past the budget warns with its size'

short=$(printf 'word %.0s' $(seq 1 1000))
plan <<EOF
# plan: symbols

## Spec

$short

## Rulings

1. Do it well.

- [ ] t1 Do the thing
      Files: a.rs
      Verify: true
EOF
check
unlike "$ERR" 'the prose is about' 'prose inside the budget does not'

## ------------------------- a Gives is the symbol path, not every word in it

plan <<'EOF'
# plan: symbols

## Spec

Do the thing.

## Rulings

1. Do it well.

- [ ] t1 Give the perms
      Files: src/perms.rs
      Gives: Perms::reject_cascade(&mut self, reason: &str) -> String (the steering line the dispatcher appends as a User entry) · HookEvent::{ToolInput(String), ToolOutput(String)}
      Verify: true
- [ ] t2 Use the ledger  [after: t1]
      Files: src/loop.rs
      Uses: Ledger::append(&mut self, kind: u8) -> u64 · BlobStore::put(bytes: Vec<u8>) -> u64
      Verify: true
EOF
check
is "$RC" 0 'a symbol nobody gives warns, it does not refuse'
unlike "$ERR" 'Gives it as' 'prose inside a Gives parenthetical is not the symbol it gives'
like "$ERR" "Uses names 'append'" 'so a Uses matching only that prose is a name nobody gives'
like "$ERR" "Uses names 'put'" 'and so is one whose name only hides inside a variant'

## ------------------------------ a Uses the tree already carries is silent

new_repo grounded
mkdir -p src
printf 'pub struct Budget { pub cents: u64 }\n' >src/kernel.rs
git add src/kernel.rs
git -c core.hooksPath=/dev/null commit -qm 'the kernel the milestones before this one left'

plan <<'EOF'
# plan: symbols

## Spec

Do the thing.

## Rulings

1. Do it well.

- [ ] t1 Give the cost model
      Files: src/cost.rs
      Gives: CostModel::predict(budget: Budget) -> u64
      Verify: true
- [ ] t2 Read the budget  [after: t1]
      Files: src/loop.rs
      Uses: Budget
      Verify: true
- [ ] t3 Read the budget elsewhere
      Files: src/report.rs
      Uses: Budget
      Verify: true
EOF
check
is "$RC" 0 'a tree-grounded Uses is no refusal'
unlike "$ERR" 'Gives it as' 'a name the tree carries is not drift from a sibling signature'
unlike "$ERR" 'may run at once' 'nor a missing edge to the task whose signature spells it'
unlike "$ERR" "Uses names 'Budget'" 'and the tree grounds it'

## ------------------------- a variant nothing calls until a later wave

new_repo cargogate
mkdir -p src
printf '[package]\nname = "gate"\nversion = "0.1.0"\n' >Cargo.toml
printf 'fn main() {}\n' >src/main.rs
git add -A
git -c core.hooksPath=/dev/null commit -qm 'a crate whose gate runs clippy'

cat >"$T_TMP/dead-variant.md" <<'EOF'
# plan: symbols

## Spec

Do the thing.

## Rulings

1. Do it well.

- [ ] t1 Give the stop
      Files: src/stop.rs
      Gives: Stop::{Done, Budget}
      Verify: true
- [ ] t2 Read the stop  [after: t1]
      Files: src/loop.rs
      Uses: Stop::Budget
      Verify: true
EOF
cp "$T_TMP/dead-variant.md" plan.md
check
is "$RC" 0 'a variant landing a wave before its caller warns, it does not refuse'
like "$ERR" "plan: task t1: Gives 'Stop::\{Done, Budget\}' and the first task using it is t2, a wave later" \
	'the giver, the variant and its first caller are named'
like "$ERR" "cargo clippy -- -D warnings" 'and the warning says what refuses it'

new_repo plaingate
cp "$T_TMP/dead-variant.md" plan.md
check
unlike "$ERR" 'cargo clippy' 'a repo with no crate has no such gate to fail'
