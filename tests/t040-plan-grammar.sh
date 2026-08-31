#!/usr/bin/env bash
# The plan grammar of spec §5.3 and the ownership evaluator of §8.7 (AC12).
# Both are exercised directly through the binary's hidden seams -- `plan-check`,
# `split-patterns` and `ownership` -- so the expectations are written down here
# rather than read back out of the implementation.
source "$(dirname -- "$0")/lib.sh"
t_init

plan_file() { cat >"$T_TMP/plan.md"; }

# The parse as JSON, the warnings on stderr, and the exit code.
parse() {
	JSON=$(workflow plan-check --json "$T_TMP/plan.md" 2>"$T_TMP/parse.err")
	RC=$?
	OUT=$(cat "$T_TMP/parse.err")
}

field() { printf '%s' "$JSON" | jq -r "$1"; }
task_field() { field ".tasks[] | select(.id == \"$1\") | .$2"; }

## ------------------------------------------------------------ round trip

plan_file <<'EOF'
# plan: cart-pricing-v2

- [x] t0 Freeze the fixture basket
      Files: tests/Fixtures/basket.json
      Verify: bin/php artisan test --filter=Fixture

- [ ] t1 Extract cart pricing into a service  [after: t0]
      Files: app/Services/Cart*.php tests/Unit/Cart*
      Verify: bin/php artisan test --filter=Cart
      Done: cart totals identical for the fixture basket
EOF
parse
is "$RC" 0 'round trip: the example parses'
is "$(field '.plan_id')" 'cart-pricing-v2' 'round trip: the plan id is the slug on the first line'
is "$(field '[.tasks[].id] | join(" ")')" 't0 t1' 'round trip: both task ids, in order'
is "$(task_field t1 title)" 'Extract cart pricing into a service' 'round trip: the title stops before [after:]'
is "$(task_field t1 'deps | join(" ")')" 't0' 'round trip: the dependency'
is "$(task_field t1 files)" 'app/Services/Cart*.php tests/Unit/Cart*' 'round trip: Files'
is "$(task_field t1 verify)" 'bin/php artisan test --filter=Cart' 'round trip: Verify'
is "$(task_field t1 done)" 'cart totals identical for the fixture basket' 'round trip: Done'
is "$(task_field t0 checked)" 'true' 'round trip: [x] means already done'
is "$(task_field t1 checked)" 'false' 'round trip: [ ] means not yet'
is "$(field '[.waves[] | join(" ")] | join(" ")')" 't0 t1' 'round trip: two waves, the dependent one second'

# The first colon-space is the split, so a colon in the value survives.
plan_file <<'EOF'
# plan: p

- [ ] t1 Do a thing
      Files: src/**
      Verify: pnpm run test:unit
EOF
parse
is "$RC" 0 'a colon inside the value parses'
is "$(task_field t1 verify)" 'pnpm run test:unit' 'the value keeps its own colon'

# Several dependencies, comma separated.
plan_file <<'EOF'
# plan: p

- [ ] a First
      Files: a
      Verify: true
- [ ] b Second
      Files: b
      Verify: true
- [ ] c Third [after: a, b]
      Files: c
      Verify: true
EOF
parse
is "$RC" 0 'a comma separated dependency list parses'
is "$(task_field c 'deps | join(" ")')" 'a b' 'both dependencies, trimmed'
is "$(field '.waves | length')" '2' 'two waves'

## ------------------------------------------------------------- rejections

plan_file <<'EOF'
- [ ] t1 No header at all
      Files: x
      Verify: true
EOF
parse
is "$RC" 1 'a plan without the "# plan:" line is refused'
like "$OUT" '# plan:' 'and says what is missing'

plan_file <<'EOF'
# plan: p

- [ ] t1 Depends on nothing that exists [after: nope]
      Files: x
      Verify: true
EOF
parse
is "$RC" 1 'an unknown dependency id is refused'
like "$OUT" 'nope' 'and names it'

plan_file <<'EOF'
# plan: p

- [ ] a One [after: b]
      Files: x
      Verify: true
- [ ] b Two [after: a]
      Files: y
      Verify: true
EOF
parse
is "$RC" 1 'a dependency cycle is refused'
like "$OUT" 'circle' 'and says the tasks wait on each other'

plan_file <<'EOF'
# plan: p

- [ ] t1 Missing its verifier
      Files: x
EOF
parse
is "$RC" 1 'a task with no Verify: is refused in the plan lane'

plan_file <<'EOF'
# plan: p

- [ ] t1 Missing its files
      Verify: true
EOF
parse
is "$RC" 1 'a task with no Files: is refused in the plan lane'

plan_file <<'EOF'
# plan: p

- [ ] t1 Two of the same key
      Files: x
      Files: y
      Verify: true
EOF
parse
is "$RC" 1 'a duplicate key is a hard error'

plan_file <<'EOF'
# plan: p

- [ ] t1 An unknown key
      Files: x
      Verify: true
      Colour: blue
EOF
parse
is "$RC" 0 'an unknown key is only a warning'
like "$OUT" 'Colour' 'and the warning names it'

# A line that means to be a task and is not one used to fall through as prose:
# the task vanished and its continuation lines attached to the task above.
plan_file <<'EOF'
# plan: p

- [ ] this-id-is-far-too-long-for-the-rule Do the thing
      Files: x
      Verify: true
- [ ] b Second
      Files: y
      Verify: true
EOF
parse
is "$RC" 1 'an id past sixteen characters is a hard error, not a dropped line'
like "$OUT" 'this-id-is-far-too-long-for-the-rule' 'and the line is quoted back'

plan_file <<'EOF'
# plan: p

- [ ] T1 Do the thing
      Files: x
      Verify: true
- [ ] b Second
      Files: y
      Verify: true
EOF
parse
is "$RC" 1 'an uppercase id is a hard error too'
like "$OUT" 'T1 Do the thing' 'and that line is quoted back as well'

plan_file <<'EOF'
# plan: p

- [ ] a Fine
      Files: x
      Verify: true
- [ ] Missing an id entirely
      Files: y
      Verify: true
EOF
parse
is "$RC" 1 'a checkbox line with no id is a hard error'

# `- [X]` is what GitHub markdown means by a ticked box, so it is one here too.
# Read as prose it took its Files: and Verify: down with it and the plan parsed
# clean with the task simply gone -- which is the M-5 failure at one more
# spelling. The task above keeps its own lines either way; the point is that
# this one exists and owns its own.
plan_file <<'EOF'
# plan: p

- [ ] t1 First
      Files: a
      Verify: true
- [X] t2 Second
      Files: b
      Verify: false
EOF
parse
is "$RC" 0 'an uppercase [X] is a ticked box'
is "$(field '[.tasks[].id] | join(" ")')" 't1 t2' 'and the task is there, not swallowed as prose'
is "$(task_field t2 checked)" 'true' 'and it counts as already done'
is "$(task_field t1 files)" 'a' 'the task above keeps its own Files:'
is "$(task_field t1 verify)" 'true' 'and its own Verify:'
is "$(task_field t2 files)" 'b' 'while the [X] task keeps its own'

# Every other checkbox spelling is refused outright, for the same reason.
plan_file <<'EOF'
# plan: p

- [ ] t1 First
      Files: a
      Verify: true
- [-] t2 A dash is not a tick
      Files: b
      Verify: false
EOF
parse
is "$RC" 1 'a box that is neither [ ] nor [x] is a hard error'
like "$OUT" '\- \[-\] t2 A dash is not a tick' 'and the line is quoted back'

# Prose that was never trying to be a task is still prose.
plan_file <<'EOF'
# plan: p

Notes: the basket fixture is frozen. - [ ] is not a task here.

- [ ] a Fine
      Files: x
      Verify: true
- [ ] b Also fine
      Files: y
      Verify: true
EOF
parse
is "$RC" 0 'a checkbox that is not at the start of a line is left alone'
is "$(field '[.tasks[].id] | join(" ")')" 'a b' 'and both real tasks are still there'

## ------------------------------------------------- Files: pattern splitting

is "$(workflow split-patterns 'app/Services/Cart*.php tests/Unit/Cart*' | tr '\n' '|')" \
	'app/Services/Cart*.php|tests/Unit/Cart*|' 'patterns split on whitespace'
is "$(workflow split-patterns 'a/b "one path/with space.php" c' | tr '\n' '|')" \
	'a/b|one path/with space.php|c|' 'a quoted pattern keeps its space'

## ------------------------------------------ the ownership fixture table

new_repo owner
mkdir -p app/Services/Cartx tests/Unit config
printf 'x\n' >app/Services/Cart.php
printf 'x\n' >app/Services/CartTotals.php
printf 'x\n' >app/Services/Cartx/Deep.php
printf 'x\n' >'app/Services/With Space.php'
printf 'x\n' >tests/Unit/CartTest.php
printf 'x\n' >.env
printf 'x\n' >config/.env.prod
base=$(git rev-parse HEAD)

# unowned <pattern>... -- what the evaluator says is not covered.
unowned() { workflow ownership --repo "$PWD" --base "$base" --branch HEAD -- "$@"; }

out=$(unowned 'app/Services/Cart*.php' 'tests/Unit/Cart*')
unlike "$out" 'app/Services/Cart\.php' 'Cart*.php owns app/Services/Cart.php'
unlike "$out" 'CartTotals' 'Cart*.php owns app/Services/CartTotals.php'
unlike "$out" 'tests/Unit/CartTest' 'tests/Unit/Cart* owns the test'
like "$out" 'Cartx/Deep.php' '* does not cross a slash: Cartx/Deep.php is not owned'
like "$out" 'With Space' 'a path with a space that nobody claimed is reported'
like "$out" '\.env' 'the root .env is not owned by a Cart pattern'

out=$(unowned '**/.env*')
unlike "$out" '\?\? \.env$' '**/.env* owns the root .env'
unlike "$out" 'config/\.env\.prod' '**/.env* owns a nested one too'
like "$out" 'Cart' 'and owns nothing else'

# Case matters here, deliberately: a case-mismatched write fails rather than
# merging, which is the safe direction (spec §9).
out=$(unowned 'app/services/cart*.php')
like "$out" 'app/Services/Cart.php' 'ownership stays case sensitive'

## ------------------------------------------ the committed side, and renames

git add -A
git -c core.hooksPath=/dev/null commit -qm 'fixtures'
base=$(git rev-parse HEAD)
git checkout -qb work
git mv app/Services/Cart.php app/Services/CartRenamed.php
git -c core.hooksPath=/dev/null commit -qm 'rename inside ownership'
out=$(workflow ownership --repo "$PWD" --base "$base" --branch work -- 'app/Services/Cart*.php')
is "$out" '' 'a rename that stays inside the patterns is owned'

git mv tests/Unit/CartTest.php tests/Unit/BasketTest.php
git -c core.hooksPath=/dev/null commit -qm 'rename out of ownership'
out=$(workflow ownership --repo "$PWD" --base "$base" --branch work -- \
	'app/Services/Cart*.php' 'tests/Unit/Cart*')
like "$out" 'BasketTest' 'a rename that lands outside the patterns is reported'
unlike "$out" 'CartRenamed' 'and the owned rename is still not reported'

# The slug becomes a branch name and a directory name.
plan_file <<'EOF2'
# plan: bad..slug

- [ ] t1 A thing
      Files: x
      Verify: true
EOF2
parse
is "$RC" 1 'a slug git would refuse as a ref is refused here'
like "$OUT" 'branch name' 'and says why'

## --------------------------- the checkout half of the check (tree findings)

# A plan is judged against the checkout it will run in, not only against its
# own grammar: a Verify that cannot pass here is refused before a worker pays
# a whole attempt discovering it (friction #DBHZBFY1), and a Files line that
# does not look like it can hold its task is warned about (friction #6485CNC0).
new_repo binonly
mkdir -p src tests
printf 'fn main() {}\n' >src/main.rs
printf '[package]\nname = "binonly"\nversion = "0.1.0"\n' >Cargo.toml
git add -A
git -c core.hooksPath=/dev/null commit -qm 'a binary-only crate'

plan_file <<'EOF2'
# plan: p

- [ ] t1 Change behaviour
      Files: src/main.rs
      Verify: cargo test --lib
EOF2
parse
is "$RC" 1 'cargo test --lib on a crate with no library target is refused'
like "$OUT" 'no library target' 'and the refusal says why'

plan_file <<'EOF2'
# plan: p

- [ ] t1 Change behaviour
      Files: src/main.rs tests/main_change.rs
      Verify: cargo test --test main_change
EOF2
parse
is "$RC" 0 'a Verify this checkout can run passes'

plan_file <<'EOF2'
# plan: p

- [ ] t1 Off in the weeds
      Files: src/verbz/answer.rs
      Verify: true
- [ ] t2 Fine
      Files: src/main.rs
      Verify: true
EOF2
parse
is "$RC" 0 'a suspect Files line is a warning, never a refusal'
like "$OUT" 'src/verbz/answer.rs' 'the pattern is named'
like "$OUT" 'typo' 'as a possible typo'

plan_file <<'EOF2'
# plan: p

- [ ] t1 Change behaviour with nothing to prove it
      Files: src/main.rs
      Verify: cargo test --test e2e
EOF2
parse
is "$RC" 0 'a missing test file is a warning too'
like "$OUT" 'no test file' 'and says what is missing'
