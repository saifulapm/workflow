#!/usr/bin/env bash
# The hook stubs: the fire condition, the depth guard, the chain, and the push
# refusal. This is AC3 of spec §12, plus the AC1 and AC5a deterministic halves.
source "$(dirname -- "$0")/lib.sh"
t_init
chmod +x "$HOOKS"/pre-commit "$HOOKS"/commit-msg "$HOOKS"/pre-push 2>/dev/null

# A repo whose suite fails: any commit that reaches the gate must be refused.
red_repo() {
	new_repo "$1"
	printf '{"name":"acme/app"}\n' >composer.json
	printf '#!/bin/sh\nexit 0\n' >artisan
	chmod +x artisan
	write_exec bin/php <<-'EOF'
		#!/bin/sh
		exit 1
	EOF
	git add -A
	git -c core.hooksPath=/dev/null commit -qm 'project files'
}

## ---------------------------------------------- fires where it should fire

# An orchestrator worktree fires on location alone, no environment needed.
mkdir -p "$XDG_STATE_HOME/workflow/worktrees/proj/plan-a"
red_repo wt
mv "$T_TMP/wt" "$XDG_STATE_HOME/workflow/worktrees/proj/plan-a/t1"
cd "$XDG_STATE_HOME/workflow/worktrees/proj/plan-a/t1" || exit 1
printf 'change\n' >>README.md
git add README.md
run git_gated commit -m 'Change the readme'
isnt "$RC" 0 'worktree: a failing suite blocks the commit'
like "$OUT" 'FAILED' 'worktree: the suite failure is what blocked it'

# A registered checkout fires only for an agent.
red_repo registered
mem_register
printf 'change\n' >>README.md
git add README.md
run env WORKFLOW_AGENT=1 git -c core.hooksPath="$HOOKS" commit -m 'Change the readme'
isnt "$RC" 0 'registered checkout + WORKFLOW_AGENT: blocked'

## ------------------------------------------ and nowhere else it should not

# The same commit by a human. `env -u WORKFLOW_AGENT` is the bypass, stated.
run env -u WORKFLOW_AGENT git -c core.hooksPath="$HOOKS" commit -m 'Change the readme'
is "$RC" 0 'human commit in the same repo: untouched'

# An unregistered scratch repo, agent environment and all: ungated. Without
# this, every test suite an agent runs would gate its own fixtures.
red_repo scratch
printf 'change\n' >>README.md
git add README.md
run env WORKFLOW_AGENT=1 git -c core.hooksPath="$HOOKS" commit -m 'Change the readme'
is "$RC" 0 'unregistered scratch repo under WORKFLOW_AGENT=1: passes ungated'

## --------------------------------------------------------------- the chain

# The repo's own hook still runs, on both paths, and the commit terminates.
red_repo chain
mkdir -p .git/hooks
write_exec .git/hooks/pre-commit <<-'EOF'
	#!/bin/sh
	echo ran >>"$(git rev-parse --show-toplevel)/own-hook-ran"
	exit 0
EOF
printf 'change\n' >>README.md
git add README.md
run env -u WORKFLOW_AGENT timeout 60 git -c core.hooksPath="$HOOKS" commit -m 'Change the readme'
is "$RC" 0 'non-firing path: the commit succeeds'
is "$(cat own-hook-ran 2>/dev/null)" 'ran' "non-firing path: the repo's own hook still ran exactly once"

red_repo chain-fire
mem_register
mkdir -p .git/hooks
write_exec .git/hooks/pre-commit <<-'EOF'
	#!/bin/sh
	echo ran >>"$(git rev-parse --show-toplevel)/own-hook-ran"
	exit 0
EOF
write_exec bin/php <<-'EOF'
	#!/bin/sh
	exit 0
EOF
printf 'change\n' >>README.md
git add README.md
run env WORKFLOW_AGENT=1 timeout 60 git -c core.hooksPath="$HOOKS" commit -m 'Change the readme'
is "$RC" 0 'firing path: a green suite lets the commit through'
is "$(cat own-hook-ran 2>/dev/null)" 'ran' "firing path: the repo's own hook ran once, and the commit terminated"

# The stub installed as the repo's own hook too: it must not exec itself.
red_repo self-chain
mem_register
mkdir -p .git/hooks
ln -sf "$HOOKS/pre-commit" .git/hooks/pre-commit
write_exec bin/php <<-'EOF'
	#!/bin/sh
	exit 0
EOF
printf 'change\n' >>README.md
git add README.md
run env WORKFLOW_AGENT=1 timeout 60 git -c core.hooksPath="$HOOKS" commit -m 'Change the readme'
is "$RC" 0 'self-chain: the stub does not exec itself and the commit terminates'

## --------------------------------------------------------- the depth guard

red_repo depth
mem_register
mkdir -p .git/hooks
write_exec .git/hooks/pre-commit <<-'EOF'
	#!/bin/sh
	echo ran >>"$(git rev-parse --show-toplevel)/own-hook-ran"
	printf '%s\n' "${WORKFLOW_HOOK_SEEN:-unset}" >"$(git rev-parse --show-toplevel)/seen"
	exit 0
EOF
printf 'change\n' >>README.md
git add README.md
seen=$(git rev-parse --path-format=absolute --git-common-dir)
run env WORKFLOW_AGENT=1 WORKFLOW_HOOK_SEEN="$seen" git -c core.hooksPath="$HOOKS" commit -m 'Change the readme'
is "$RC" 0 'depth guard: a second entry for the same repo skips the failing check'
is "$(cat own-hook-ran 2>/dev/null)" 'ran' 'depth guard: skipping the check still chains'

rm -f own-hook-ran seen
printf 'more\n' >>README.md
git add README.md
run env WORKFLOW_AGENT=1 WORKFLOW_HOOK_SEEN=/some/other/repo/.git git -c core.hooksPath="$HOOKS" commit -m 'Change the readme'
isnt "$RC" 0 'depth guard: another repo in the variable does not excuse this one'

rm -f own-hook-ran seen
run env -u WORKFLOW_AGENT git -c core.hooksPath="$HOOKS" commit -m 'Change the readme'
is "$RC" 0 'non-firing path: the commit goes through'
is "$(cat seen 2>/dev/null)" 'unset' 'non-firing path: WORKFLOW_HOOK_SEEN is never exported'

## -------------------------------------------------------- the husky shadow

# A repo-local core.hooksPath beats the global one, and the gate is simply not
# there. This is the hole doctor has to report; the stub cannot close it.
red_repo husky
mem_register
git config --global core.hooksPath "$HOOKS" # the install this project asks for
printf 'change\n' >>README.md
git add README.md
run env WORKFLOW_AGENT=1 git commit -m 'Change the readme'
isnt "$RC" 0 'a global core.hooksPath gates the repo with no -c needed'

mkdir -p .husky
write_exec .husky/pre-commit <<-'EOF'
	#!/bin/sh
	exit 0
EOF
git config core.hooksPath .husky
run env WORKFLOW_AGENT=1 git commit -m 'Change the readme'
is "$RC" 0 'repo-local core.hooksPath beats the global one: the gate is gone'
git config --global --unset core.hooksPath

## ------------------------------------------------------------ commit-msg

red_repo msg
mem_register
write_exec bin/php <<-'EOF'
	#!/bin/sh
	exit 0
EOF
printf 'change\n' >>README.md
git add README.md
run env WORKFLOW_AGENT=1 git -c core.hooksPath="$HOOKS" commit -m 'Change the readme

Co-Authored-By: Claude <noreply@anthropic.com>'
isnt "$RC" 0 'commit-msg: a provenance trailer is refused'
run env WORKFLOW_AGENT=1 git -c core.hooksPath="$HOOKS" commit -m 'Change the readme'
is "$RC" 0 'commit-msg: the ordinary message goes through'
unlike "$(git log -1 --format=%B)" 'Co-Authored-By' 'commit-msg: no trailer survives into history'

## -------------------------------------------------------- partial commits

red_repo partial
mem_register
write_exec bin/php <<-'EOF'
	#!/bin/sh
	exit 0
EOF
mkdir -p tests
cat >tests/CartTest.php <<-'EOF'
	<?php
	class CartTest
	{
	    public function testCartTotals() { }
	}
EOF
printf 'x\n' >other.txt
git add -A
git -c core.hooksPath=/dev/null commit -qm 'tests and other'
rm tests/CartTest.php
printf 'y\n' >>other.txt
# `git commit --only` builds a temporary index holding just these paths.
run env WORKFLOW_AGENT=1 git -c core.hooksPath="$HOOKS" commit --only tests/CartTest.php -m 'Drop the cart test'
isnt "$RC" 0 'partial commit: the test deletion in the temporary index is caught'
like "$OUT" 'testCartTotals' 'partial commit: the gate names the test it saw go'

## -------------------------------------------------------------- pre-push

git init -q --bare "$T_TMP/origin.git"
red_repo pusher
mem_register
git remote add origin "$T_TMP/origin.git"
run env WORKFLOW_AGENT=1 git -c core.hooksPath="$HOOKS" push -q origin HEAD:refs/heads/main
isnt "$RC" 0 'pre-push: an agent push in a registered checkout is refused'
like "$OUT" 'requires explicit approval' 'pre-push: says what it wants'

run env WORKFLOW_AGENT=1 WORKFLOW_ALLOW_PUSH=1 git -c core.hooksPath="$HOOKS" push -q origin HEAD:refs/heads/main
is "$RC" 0 'pre-push: WORKFLOW_ALLOW_PUSH=1 releases it'

run env -u WORKFLOW_AGENT git -c core.hooksPath="$HOOKS" push -q origin HEAD:refs/heads/human
is "$RC" 0 'pre-push: a human push is untouched'

# A bare repo has no work tree: step 1 lets it straight through.
git clone -q --bare "$T_TMP/pusher" "$T_TMP/bare.git"
cd "$T_TMP/bare.git" || exit 1
run env WORKFLOW_AGENT=1 git -c core.hooksPath="$HOOKS" push -q "$T_TMP/origin.git" main:refs/heads/from-bare
is "$RC" 0 'pre-push: a push from a bare clone succeeds'
