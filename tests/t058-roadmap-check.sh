#!/usr/bin/env bash
# `workflow plan-check` on a roadmap: every milestone's plan is read from
# <dir>/<id>.md beside it and judged in wave order, each against the tree plus
# what the milestones it waits on, transitively, write and give.
source "$(dirname -- "$0")/lib.sh"
t_init

new_repo roadmap
mkdir -p engine/src
printf 'pub fn boot() {}\n' >engine/src/lib.rs
git add engine/src/lib.rs
git -c core.hooksPath=/dev/null commit -qm 'the engine'

# The roadmap and its milestone plans live beside each other, the way a
# planner writes them before storing them in mem.
mkdir -p road
road() { cat >"road/roadmap.md"; }
milestone() { cat >"road/$1.md"; }
road_reset() { rm -rf road && mkdir road; }
# The report on stdout, the findings on stderr, and the exit code.
check() {
	OUT=$(workflow plan-check road/roadmap.md 2>"$T_TMP/check.err")
	RC=$?
	ERR=$(cat "$T_TMP/check.err")
}

## --------------------------------------------------- a chain that holds

# Listed out of the order they run in: the walk is by wave, so a milestone is
# read after everything it waits on, however the document lists them.
road <<'EOF'
# roadmap: shop

- [ ] m4-search Search
- [ ] m3-reports Reports  [after: m2-billing]
- [ ] m2-billing Billing  [after: m1-auth]
- [ ] m1-auth Sign-in and sessions
EOF

milestone m1-auth <<'EOF'
# plan: m1-auth

Sign-in issues a session and boot reads it back.

- [ ] t1 Add the session store
      Files: engine/auth/session.rs
      Gives: fn sign_in(user: &str) -> Session
      Verify: cargo build
- [ ] t2 Read sessions at boot  [after: t1]
      Files: engine/src/lib.rs
      Read: engine/auth/session.rs
      Uses: fn sign_in(user: &str) -> Session
      Verify: cargo build
EOF

milestone m2-billing <<'EOF'
# plan: m2-billing

A signed-in customer is charged and can be refunded.

- [ ] t1 Charge a signed-in customer
      Files: engine/auth/charge.rs
      Read: engine/auth/session.rs
      Uses: fn sign_in(user: &str) -> Session
      Verify: cargo build
- [ ] t2 Refund a charge  [after: t1]
      Files: engine/src/refund.rs
      Verify: cargo build
EOF

milestone m3-reports <<'EOF'
# plan: m3-reports

Sessions are reported on and the reports are summarised.

- [ ] t1 Report on sessions
      Files: engine/src/report.rs
      Read: engine/auth/session.rs
      Uses: fn sign_in(user: &str) -> Session
      Verify: cargo build
- [ ] t2 Summarise the reports  [after: t1]
      Files: engine/src/summary.rs
      Pattern: engine/auth/session.rs
      Verify: cargo build
EOF

milestone m4-search <<'EOF'
# plan: m4-search

Orders are searched and indexed.

- [ ] t1 Search the orders
      Files: engine/src/search.rs
      Read: engine/auth/session.rs
      Uses: fn sign_in(user: &str) -> Session
      Verify: cargo build
- [ ] t2 Index the orders  [after: t1]
      Files: engine/src/index.rs
      Pattern: engine/auth/session.rs
      Verify: cargo build
EOF

check
is "$RC" 0 'a roadmap whose milestones all hold is not refused'
like "$OUT" '^roadmap: shop$' 'the report still names the roadmap'
like "$ERR" "m1-auth: plan: task t1: 'engine/auth/session.rs' matches nothing" \
	'a finding carries the milestone it came from'
like "$ERR" 'milestone m1-auth: reading road/m1-auth.md' \
	'and every milestone names the file it was read from'
unlike "$ERR" 'm2-billing: plan:' \
	'what a milestone it waits on writes and gives is not missing from it'
unlike "$ERR" 'm3-reports: plan:' \
	'and the chain reaches through the milestone between them'
like "$ERR" 'm4-search: plan: task t1: Read names' \
	'reading what a milestone it does not wait for writes still warns'
like "$ERR" 'm4-search: plan: task t1: Uses names' \
	'and so does using what that milestone gives'
like "$ERR" 'm4-search: plan: task t2: Pattern points at' \
	'a Pattern is read the same way'

## ------------------------------------------- the milestone plans themselves

# A milestone names a plan; no plan under that name is the roadmap pointing at
# nothing.
road_reset
road <<'EOF'
# roadmap: shop

- [ ] m1-auth Sign-in and sessions
- [ ] m2-billing Billing  [after: m1-auth]
EOF
milestone m1-auth <<'EOF'
# plan: m1-auth

- [ ] t1 Add the session store
      Files: engine/src/session.rs
      Verify: cargo build
- [ ] t2 Read sessions at boot  [after: t1]
      Files: engine/src/lib.rs
      Verify: cargo build
EOF
check
is "$RC" 0 'a later milestone with no plan beside the roadmap is not refused'
like "$ERR" 'm2-billing: no plan at road/m2-billing.md.*picked up' \
	'but it is named, with when its plan is cut'
# The next milestone up is the one the roadmap is stored with a plan for.
rm road/m1-auth.md
check
is "$RC" 1 'the first open milestone with no plan is refused'
like "$ERR" 'm1-auth: no plan at road/m1-auth.md' 'and the refusal names the file it looked for'
# A landed milestone's plan has gone into the tree; nothing is read for it.
road <<'EOF2'
# roadmap: shop

- [x] m1-auth Sign-in and sessions
- [ ] m2-billing Billing  [after: m1-auth]
      Show: Saiful pays a test order from the cart and sees it marked paid
EOF2
milestone m2-billing <<'EOF2'
# plan: m2-billing

- [ ] t1 Charge a customer
      Files: engine/src/billing.rs
      Verify: cargo build
- [ ] t2 Mark the order paid  [after: t1]
      Files: engine/src/lib.rs
      Verify: cargo build
EOF2
check
is "$RC" 0 'a ticked milestone with no plan is passed over'
unlike "$ERR" 'm1-auth' 'and nothing is said about it'
unlike "$ERR" 'm2-billing: no Show' 'a milestone with a Show line is not asked for one'
# Every open milestone says what Saiful sees once it lands; a milestone whose
# only claim is a Done line is checked from a diff, not from the product.
road <<'EOF2'
# roadmap: shop

- [ ] m1-auth Sign-in and sessions
- [ ] m2-billing Billing  [after: m1-auth]
EOF2
milestone m1-auth <<'EOF2'
# plan: m1-auth

- [ ] t1 Add the session store
      Files: engine/src/session.rs
      Verify: cargo build
- [ ] t2 Read sessions at boot  [after: t1]
      Files: engine/src/lib.rs
      Verify: cargo build
EOF2
check
like "$ERR" 'milestone m1-auth: no Show line' 'a milestone without a Show line is named'
like "$ERR" 'milestone m2-billing: no Show line' 'each of them'

# A plan filed under another milestone's name would send a run at the wrong one.
milestone m2-billing <<'EOF'
# plan: billing

- [ ] t1 Charge a customer
      Files: engine/src/charge.rs
      Verify: cargo build
- [ ] t2 Refund a charge  [after: t1]
      Files: engine/src/refund.rs
      Verify: cargo build
EOF
check
is "$RC" 1 'a plan headed with a slug that is not the milestone id is refused'
like "$ERR" "m2-billing.*'# plan: billing'" 'and the refusal names both'

# The header word matters as much as the slug: a file headed as a roadmap is
# asked for no Files: and no Verify: when it parses, so a milestone filed as
# one would go to run with neither.
milestone m2-billing <<'EOF'
# roadmap: m2-billing

- [ ] t1 Charge a customer
- [ ] t2 Refund a charge  [after: t1]
EOF
check
is "$RC" 1 'a milestone plan headed as a roadmap is refused'
like "$ERR" "m2-billing.*'# roadmap: m2-billing'" 'and the refusal names the header it found'

# Files: and Verify: are required of every task in a milestone's plan: it is
# dispatched as it stands.
milestone m2-billing <<'EOF'
# plan: m2-billing

- [ ] t1 Charge a customer
      Files: engine/src/charge.rs
- [ ] t2 Refund a charge  [after: t1]
      Files: engine/src/refund.rs
      Verify: cargo build
EOF
check
is "$RC" 1 'a milestone plan whose task has no Verify: is refused'
like "$ERR" 'task t1 has no Verify' 'the grammar says what is wrong with it'
like "$ERR" 'm2-billing.*does not parse' 'and the refusal names the milestone'

# run refuses a one-task plan, so a milestone cut down to one is worth saying
# out loud -- but the roadmap is not wrong, so it warns.
milestone m2-billing <<'EOF'
# plan: m2-billing

- [ ] t1 Charge a customer
      Files: engine/src/charge.rs
      Verify: cargo build
EOF
check
is "$RC" 0 'a one-task milestone is not refused'
like "$ERR" 'm2-billing.*one task' 'but it is named'
like "$ERR" 'run refuses' 'with what run would do with it'

## ------------------------------------------- whose grammar line is whose

# The grammar prints its own diagnostics as it reads a milestone, long before
# the findings are batched and printed together, so two broken milestones would
# run their lines into one heap. What is read is announced first, and the lines
# under an announcement are that milestone's.
road_reset
road <<'EOF'
# roadmap: shop

- [ ] m1-auth Sign-in and sessions
- [ ] m2-billing Billing  [after: m1-auth]
EOF
milestone m1-auth <<'EOF'
# plan: m1-auth

- [ ] t1 Add the session store
      Files: engine/src/session.rs
- [ ] t2 Read sessions at boot  [after: t1]
      Files: engine/src/lib.rs
      Verify: cargo build
EOF
milestone m2-billing <<'EOF'
# plan: m2-billing

- [ ] t1 Charge a customer
      Verify: cargo build
- [ ] t2 Refund a charge  [after: t1]
      Files: engine/src/refund.rs
      Verify: cargo build
EOF
# What stderr said after one milestone was announced and before the next was.
under() {
	awk -v id="$1" '
		$0 ~ ": milestone " id ": reading " { on = 1; next }
		/: milestone [^ ]+: reading / { on = 0 }
		on' "$T_TMP/check.err"
}
check
is "$RC" 1 'two milestones that do not parse are both refused'
like "$(under m1-auth)" 'task t1 has no Verify' \
	"the first milestone's grammar lines sit under its own name"
unlike "$(under m1-auth)" 'task t1 has no Files' 'and not the next one'
like "$(under m2-billing)" 'task t1 has no Files' "the second milestone's sit under its"

## ------------------------------------------------------------ a plain plan

# Nothing of the roadmap walk reaches a plan: it is judged against the tree
# alone, exactly as before.
road_reset
cat >road/plan.md <<'EOF'
# plan: p

- [ ] t1 Add the session store
      Files: engine/src/session.rs
      Read: engine/auth/session.rs
      Verify: cargo build
EOF
OUT=$(workflow plan-check road/plan.md 2>&1)
RC=$?
is "$RC" 0 'a plan is checked as it always was'
like "$OUT" '^plan: p$' 'and reported as a plan'
like "$OUT" 'plan: task t1: Read names' 'with its findings unprefixed'
