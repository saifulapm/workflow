#!/usr/bin/env bash
# The hook stubs: the fire condition, the chain and the push refusal.
# pre-commit and commit-msg run the hygiene check and nothing else -- no
# suite -- for humans and agents alike.
source "$(dirname -- "$0")/lib.sh"
t_init
chmod +x "$HOOKS"/pre-commit "$HOOKS"/commit-msg "$HOOKS"/pre-push 2>/dev/null

# A repo whose suite fails. The hooks never run a suite, so a clean commit must
# go through it.
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

# dirty -- stage a new file with a line the hygiene hard tier refuses.
n=0
dirty() {
	n=$((n + 1))
	printf 'fn main() {}\n// cached because ruling 4 says so\n' >"cache$n.rs"
	git add "cache$n.rs"
}

# clean -- stage a change with nothing to refuse.
clean() {
	printf 'change\n' >>README.md
	git add README.md
}

## ---------------------------------------------- fires where it should fire

# A registered checkout fires only for an agent, and what it reads is the staged diff.
red_repo registered
mem_register
"$MEM_BIN" project set verify false >/dev/null
dirty
run env WORKFLOW_AGENT=1 git -c core.hooksPath="$HOOKS" commit -m 'Cache the lookup'
isnt "$RC" 0 'registered checkout + WORKFLOW_AGENT: a refused line blocks the commit'
like "$OUT" '^cache[0-9]+\.rs:2: hard numbered ruling: ' 'registered checkout + WORKFLOW_AGENT: hygiene is what blocked it'

# pi has no `env` key in its settings, so a hand-started pi session carries no
# WORKFLOW_AGENT. PI_CODING_AGENT is pi's own marker, exported to everything it
# starts, and the hook reads it the same way.
run env -u WORKFLOW_AGENT PI_CODING_AGENT=true git -c core.hooksPath="$HOOKS" commit -m 'Cache the lookup'
isnt "$RC" 0 'registered checkout + PI_CODING_AGENT: blocked too'

# The same commit by a human is held to hygiene as well.
run env -u WORKFLOW_AGENT -u PI_CODING_AGENT git -c core.hooksPath="$HOOKS" commit -m 'Cache the lookup'
isnt "$RC" 0 'human commit in the same repo: held to hygiene too'

# Nothing else is read: a clean commit goes through even though the project's
# own suite is red, for an agent and for a human.
git reset -q
rm -f cache*.rs
clean
run env WORKFLOW_AGENT=1 git -c core.hooksPath="$HOOKS" commit -m 'Change the readme'
is "$RC" 0 'agent: a clean commit goes through a red suite, no suite is run'
clean
run env -u WORKFLOW_AGENT -u PI_CODING_AGENT git -c core.hooksPath="$HOOKS" commit -m 'Change the readme again'
is "$RC" 0 'human: a clean commit goes through a red suite too'

## ------------------------------------------ and nowhere else it should not

# An unregistered scratch repo, agent environment and all: ungated. Without
# this, every test suite an agent runs would gate its own fixtures.
red_repo scratch
dirty
run env WORKFLOW_AGENT=1 git -c core.hooksPath="$HOOKS" commit -m 'Cache the lookup'
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
clean
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
clean
run env WORKFLOW_AGENT=1 timeout 60 git -c core.hooksPath="$HOOKS" commit -m 'Change the readme'
is "$RC" 0 'firing path: a clean diff lets the commit through'
is "$(cat own-hook-ran 2>/dev/null)" 'ran' "firing path: the repo's own hook ran once, and the commit terminated"

# The stub installed as the repo's own hook too: it must not exec itself.
red_repo self-chain
mem_register
mkdir -p .git/hooks
ln -sf "$HOOKS/pre-commit" .git/hooks/pre-commit
clean
run env WORKFLOW_AGENT=1 timeout 60 git -c core.hooksPath="$HOOKS" commit -m 'Change the readme'
is "$RC" 0 'self-chain: the stub does not exec itself and the commit terminates'

## -------------------------------------------------------- the husky shadow

# A repo-local core.hooksPath beats the global one, and the hook is simply not
# there. This is a hole the stub cannot close.
red_repo husky
mem_register
git config --global core.hooksPath "$HOOKS" # the install this project asks for
dirty
run env WORKFLOW_AGENT=1 git commit -m 'Cache the lookup'
isnt "$RC" 0 'a global core.hooksPath gates the repo with no -c needed'

mkdir -p .husky
write_exec .husky/pre-commit <<-'EOF'
	#!/bin/sh
	exit 0
EOF
git config core.hooksPath .husky
run env WORKFLOW_AGENT=1 git commit -m 'Cache the lookup'
is "$RC" 0 'repo-local core.hooksPath beats the global one: the hook is gone'
git config --global --unset core.hooksPath

## ------------------------------------------------------------ commit-msg

red_repo msg
mem_register
clean
run env WORKFLOW_AGENT=1 git -c core.hooksPath="$HOOKS" commit -m 'Change the readme

Co-Authored-By: Claude <noreply@anthropic.com>'
isnt "$RC" 0 'commit-msg: a provenance trailer is refused'
run env WORKFLOW_AGENT=1 git -c core.hooksPath="$HOOKS" commit -m 'Change the readme'
is "$RC" 0 'commit-msg: the ordinary message goes through'
unlike "$(git log -1 --format=%B)" 'Co-Authored-By' 'commit-msg: no trailer survives into history'

## -------------------------------------------------------- partial commits

red_repo partial
mem_register
printf 'fn main() {}\n' >cache.rs
git add cache.rs
git -c core.hooksPath=/dev/null commit -qm 'Add the cache'
# Nothing is staged in the real index; the line lives in the working tree only.
# `git commit --only` builds a temporary index holding just that path, which is
# the one the hook has to read.
printf 'fn main() {}\n// cached because ruling 4 says so\n' >cache.rs
run env WORKFLOW_AGENT=1 git -c core.hooksPath="$HOOKS" commit --only cache.rs -m 'Cache the lookup'
isnt "$RC" 0 'partial commit: the line in the temporary index is caught'
like "$OUT" '^cache\.rs:2: hard numbered ruling: ' 'partial commit: the hook names the line it saw'

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
