#!/usr/bin/env bash
# workflow verify: detection, the polyglot rule, the no-verifier ruling flow,
# the child-process scrub and the exit contract of spec §7.
source "$(dirname -- "$0")/lib.sh"
t_init

# A PHP project whose verifier is a shim we control: composer.json + artisan
# make verify choose `bin/php artisan test`, and bin/php records how it was
# called and what environment it was handed.
php_project() {
	new_repo "$1"
	printf '{"name":"acme/app"}\n' >composer.json
	printf '#!/bin/sh\nexit 0\n' >artisan
	chmod +x artisan
	write_exec bin/php <<-'EOF'
		#!/bin/sh
		printf '%s\n' "$*" >>"$PWD/php-calls"
		env >"$PWD/php-env"
		exit "${FAKE_PHP_EXIT:-0}"
	EOF
	git add -A
	git -c core.hooksPath=/dev/null commit -qm 'project files'
}

## ---------------------------------------------------------------- detection

php_project php
run workflow verify
is "$RC" 0 'php project: passing shim verifier exits 0'
like "$OUT" 'bin/php artisan test' 'php project: runs artisan test through the bin/php shim'
is "$(cat php-calls)" 'artisan test' 'php project: the shim really ran'

run env FAKE_PHP_EXIT=1 workflow verify
is "$RC" 1 'php project: failing suite exits 1'

# The child suite is scrubbed of the git and workflow variables (spec §7).
run env GIT_DIR="$PWD/.git" GIT_INDEX_FILE="$PWD/.git/index" GIT_PREFIX=x/ \
	WORKFLOW_AGENT=1 WORKFLOW_HOOK_SEEN=/somewhere workflow verify
is "$RC" 0 'scrub: verify still passes with the git environment inherited'
unlike "$(cat php-env)" '^(GIT_DIR|GIT_INDEX_FILE|GIT_PREFIX|WORKFLOW_AGENT|WORKFLOW_HOOK_SEEN)=' \
	'scrub: the child suite sees none of the five scrubbed variables'

# Run from a subdirectory: verify works from the toplevel.
mkdir -p deep/er
cd deep/er || exit 1
run workflow verify
is "$RC" 0 'subdirectory: verify resolves the toplevel and passes'
cd "$T_TMP/php" || exit 1

new_repo node
printf '{"name":"n","scripts":{"test":"exit 0"}}\n' >package.json
run workflow verify
is "$RC" 0 'node project with a test script: exits 0'
like "$OUT" 'pnpm' 'node project: the runner is pnpm'

printf '{"name":"n","scripts":{"build":"true"}}\n' >package.json
run workflow verify
is "$RC" 2 'node project without a test script: no verifier, exit 2'

printf '{"name":"n","scripts":{"test":"echo \\"Error: no test specified\\" && exit 1"}}\n' >package.json
run workflow verify
is "$RC" 2 'node project with the npm placeholder test script: no verifier, exit 2'

new_repo rust
printf '[package]\nname = "probe"\nversion = "0.0.0"\nedition = "2021"\n' >Cargo.toml
mkdir -p src
printf '#[test]\nfn passes() {\n    assert_eq!(2 + 2, 4);\n}\n' >src/lib.rs
run workflow verify
is "$RC" 0 'rust floor: a clean crate passes'
like "$OUT" 'cargo test && cargo clippy -- -D warnings && cargo fmt --check' \
	'rust floor: the floor is the whole three-part command'

# The two halves of the floor that are not `cargo test` really do run.
printf '\npub fn ugly()   {\n    let _x = 1;\n}\n' >>src/lib.rs
run workflow verify
is "$RC" 1 'rust floor: formatting the crate does not agree with is a failure'
printf '#[test]\nfn passes() {\n    assert_eq!(2 + 2, 4);\n}\n\npub fn takes(s: &String) -> usize {\n    s.len()\n}\n' >src/lib.rs
run workflow verify
is "$RC" 1 'rust floor: a clippy lint is a failure, warnings being denied'

new_repo theme
mkdir -p config layout
printf '{}\n' >config/settings_schema.json
printf 'x\n' >layout/theme.liquid
write_exec "$T_TMP/bin/theme-check" <<-'EOF'
	#!/bin/sh
	exit 0
EOF
run workflow verify
is "$RC" 0 'shopify theme: theme-check runs and passes'
like "$OUT" 'verify theme: theme-check' 'shopify theme: theme-check is the detected verifier'
rm -f "$T_TMP/bin/theme-check"

## ------------------------------------------------- tier 2: declared entry points

# A stub composer, so what runs is observable and nothing reaches the network.
write_exec "$T_TMP/bin/composer" <<-'EOF'
	#!/bin/sh
	printf '%s\n' "$*" >>"$PWD/composer-calls"
	exit "${FAKE_COMPOSER_EXIT:-0}"
EOF

php_project composer-script
printf '{"name":"acme/app","scripts":{"test":"phpunit"}}\n' >composer.json
git add composer.json
git -c core.hooksPath=/dev/null commit -qm 'declare the test script'
run workflow verify
is "$RC" 0 'composer scripts.test: composer test runs'
like "$OUT" 'composer test' 'composer scripts.test: named in the output'
is "$(cat composer-calls)" 'test' 'composer scripts.test: the stub really ran'
is "$([ -f php-calls ] && cat php-calls)" '' \
	'tier separation: a declared composer script beats the artisan floor'
unlike "$OUT" 'artisan' 'tier separation: the floor is not even mentioned'

run env FAKE_COMPOSER_EXIT=1 workflow verify
is "$RC" 1 'composer scripts.test: a red script is a failed verify'

new_repo just-project
printf '[package]\nname = "j"\nversion = "0.0.0"\nedition = "2021"\n' >Cargo.toml
printf 'test:\n\ttouch just-ran\n' >justfile
run workflow verify
is "$RC" 0 'justfile test recipe: just test runs'
like "$OUT" 'just test' 'justfile: named in the output'
truthy "$([ -f just-ran ] && printf 0 || printf 1)" 'justfile: the recipe really ran'
unlike "$OUT" 'cargo' 'tier separation: a repo-wide just test replaces the ecosystem floor'

new_repo just-near-miss
printf 'test-unit:\n\ttouch wrong-recipe\n' >justfile
run workflow verify
is "$RC" 2 'justfile: a recipe merely starting with "test" is not the test target'
is "$([ -f wrong-recipe ] && printf yes || printf no)" no 'justfile: and it was not run'

new_repo make-project
printf 'test:\n\t@touch make-ran\n' >Makefile
run workflow verify
is "$RC" 0 'Makefile test target: make test runs'
like "$OUT" 'make test' 'Makefile: named in the output'
truthy "$([ -f make-ran ] && printf 0 || printf 1)" 'Makefile: the target really ran'

new_repo node-lint
printf '{"name":"n","scripts":{"test":"touch test-ran","lint":"touch lint-ran"}}\n' >package.json
run workflow verify
is "$RC" 0 'package.json with test and lint: both pass'
truthy "$([ -f test-ran ] && printf 0 || printf 1)" 'node: pnpm run test ran'
truthy "$([ -f lint-ran ] && printf 0 || printf 1)" 'node: pnpm lint ran too, being scripted'

printf '{"name":"n","scripts":{"test":"touch test-ran"}}\n' >package.json
rm -f test-ran lint-ran
run workflow verify
is "$RC" 0 'package.json without a lint script: still fine'
is "$([ -f lint-ran ] && printf yes || printf no)" no 'node: nothing invents a lint script'

## ---------------------------------------------------- tier 1: what mem records

php_project override
mem_register
"$MEM_BIN" project set verify 'touch override-ran' >/dev/null
run workflow verify
is "$RC" 0 "project override: mem's command is what runs"
like "$OUT" 'verify project: touch override-ran' 'project override: named in the output'
truthy "$([ -f override-ran ] && printf 0 || printf 1)" 'project override: it really ran'
is "$([ -f php-calls ] && cat php-calls)" '' 'project override: the floor never ran'

# It short-circuits a declared entry point too, not only the floor.
printf '{"name":"acme/app","scripts":{"test":"phpunit"}}\n' >composer.json
rm -f override-ran
run workflow verify
is "$RC" 0 'project override: still exits 0 with a composer script present'
truthy "$([ -f override-ran ] && printf 0 || printf 1)" 'project override: ran again'
is "$([ -f composer-calls ] && cat composer-calls)" '' \
	'project override: it short-circuits the declared entry points as well'

"$MEM_BIN" project set verify 'exit 7' >/dev/null
run workflow verify
is "$RC" 1 'project override: a red override is a failed verify, not a missing one'

## ----------------------------------------------------------------- polyglot

php_project poly
printf '{"name":"n","scripts":{"test":"exit 3"}}\n' >package.json
run workflow verify
is "$RC" 1 'polyglot: one green and one red suite exits 1'
like "$OUT" 'pnpm' 'polyglot: the node suite is named in the output'
is "$(cat php-calls)" 'artisan test' 'polyglot: the green suite ran too'

## -------------------------------------------------------- no verifier flow

new_repo bare-of-tests
run workflow verify
is "$RC" 2 'no verifier: exit 2'
like "$OUT" 'mem save --kind ruling --type no-verifier' 'no verifier: prints the exact ruling command'

mem_register
"$MEM_BIN" save --kind ruling --type no-verifier \
	'No suite in this repo yet - scratch checkout - cost if wrong: an unverified commit' >/dev/null
run workflow verify
is "$RC" 0 'no verifier with a ruling recorded: downgraded to a warning, exit 0'
like "$OUT" '[Ww]arn' 'no verifier with a ruling: says it is a warning'

# Location decides the lane: under the worktrees root it is the plan lane and a
# missing verifier is a hard failure, ruling or no ruling.
wt="$XDG_STATE_HOME/workflow/worktrees/proj/plan-x/t1"
mkdir -p "$wt"
cd "$wt" || exit 1
git init -q .
printf 'x\n' >f
git add f
git -c core.hooksPath=/dev/null commit -qm seed
mem_register
"$MEM_BIN" save --kind ruling --type no-verifier 'irrelevant here' >/dev/null
run workflow verify
is "$RC" 2 'plan lane (worktrees root): no verifier is a hard failure'
like "$OUT" '[Pp]lan lane' 'plan lane: the message names the lane'

## --------------------------------------- the task's own command, in a worktree

# Inside a run worktree the repo-wide suite may need sibling tasks that are not
# here yet (the whole-suite deadlock): the gate holds the task to ITS Verify
# command, which dispatch records in the run dir. The merge gate stays on the
# full suite.
wt="$XDG_STATE_HOME/workflow/worktrees/proj/plan-y/t7"
mkdir -p "$wt"
cd "$wt" || exit 1
git init -q .
printf '{"name":"p/wt"}\n' >composer.json
printf '#!/bin/sh\nexit 0\n' >artisan
chmod +x artisan
write_exec bin/php <<-'EOF'
	#!/bin/sh
	exit 1
EOF
git add -A
git -c core.hooksPath=/dev/null commit -qm seed
mkdir -p "$XDG_STATE_HOME/workflow/runs/proj/plan-y"
printf 'touch task-verify-ran\n' >"$XDG_STATE_HOME/workflow/runs/proj/plan-y/t7.verify"

run workflow verify
is "$RC" 0 'task worktree: the task Verify decides, and the red repo suite does not run'
if [ -f task-verify-ran ]; then
	ok 'task worktree: the task command actually ran'
else
	notok 'task worktree: the task command actually ran'
fi
like "$OUT" 'task' 'task worktree: the output names the task command'

run workflow verify --gate
is "$RC" 1 'the merge gate is exempt: the red repo suite decides there, not the green task command'

printf 'exit 1\n' >"$XDG_STATE_HOME/workflow/runs/proj/plan-y/t7.verify"
run workflow verify
is "$RC" 1 'task worktree: a red task command fails the gate'

rm "$XDG_STATE_HOME/workflow/runs/proj/plan-y/t7.verify"
run workflow verify
is "$RC" 1 'no recorded task command: the repo suite decides again'

## ------------------------------------------------------------- green cache

php_project cache
mem_register
run workflow verify --hook
is "$RC" 0 'green cache: first hook run is green'
before=$(wc -l <php-calls)
run workflow verify --hook
is "$RC" 0 'green cache: second hook run on the same staged tree is green'
is "$(wc -l <php-calls)" "$before" 'green cache: the second hook run skipped the suite'
like "$OUT" '[Cc]ached' 'green cache: says the tree was already green'

# A direct (non-hook) invocation never consults the cache: that is what an
# agent runs against a working tree the index has not seen yet.
run workflow verify
isnt "$(wc -l <php-calls)" "$before" 'green cache: a direct invocation still runs the suite'

# A new staged tree invalidates it.
printf 'change\n' >>README.md
git add README.md
before=$(wc -l <php-calls)
run workflow verify --hook
isnt "$(wc -l <php-calls)" "$before" 'green cache: a different staged tree runs the suite again'

## ----------------------------------------------------------------- opt-out

php_project optout
mem_register
"$MEM_BIN" save --kind ruling --type verify-optout 'suite is too slow in this repo' >/dev/null
run workflow verify --hook
is "$RC" 0 'verify-optout: the hook path exits 0'
is "$([ -f php-calls ] && cat php-calls)" '' 'verify-optout: the hook path did not run the suite'
run workflow verify
is "$RC" 0 'verify-optout: a direct invocation still runs the suite'
is "$(cat php-calls)" 'artisan test' 'verify-optout: never downgrades the authoritative path'
