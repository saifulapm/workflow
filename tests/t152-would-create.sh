#!/usr/bin/env bash
# workflow hygiene --would-create: a new agent instruction file in a checkout
# mem knows is refused, in a linked worktree too; a source file, a path under
# a tool's own directory, a file that exists and any path in a repository mem
# does not know pass. --staged --known reads nothing where mem knows nothing.
source "$(dirname -- "$0")/lib.sh"
t_init

new_repo app
mem_register
app=$PWD

## ------------------------------------------------------------- registered

run workflow hygiene --would-create CLAUDE.md
is "$RC" 1 'registered: a new CLAUDE.md exits 1'
is "$OUT" 'CLAUDE.md:0: hard agent file: this belongs in mem' 'registered: with the one line'

run workflow hygiene --would-create "$app/docs/new/.claude/skills/x/SKILL.md"
is "$RC" 1 'registered: an absolute path under a missing directory exits 1'
is "$OUT" 'docs/new/.claude/skills/x/SKILL.md:0: hard agent file: this belongs in mem' \
	'registered: named relative to the top'

mkdir -p src
cd src || exit 1
run workflow hygiene --would-create AGENTS.md
is "$RC:$OUT" '1:src/AGENTS.md:0: hard agent file: this belongs in mem' 'registered: from a subdirectory'
cd "$app" || exit 1

run workflow hygiene --would-create src/new.rs
is "$RC:$OUT" '0:' 'allowed: a source file'
run workflow hygiene --would-create .amx/x/CLAUDE.md
is "$RC:$OUT" '0:' 'allowed: under a tool directory'
printf 'seed\n' >CLAUDE.md
run workflow hygiene --would-create CLAUDE.md
is "$RC:$OUT" '0:' 'allowed: a CLAUDE.md that exists'

git add -f CLAUDE.md
run workflow hygiene --staged --known
is "$RC" 1 '--known: a known checkout is still read'
git rm -q --cached CLAUDE.md
rm CLAUDE.md

## ---------------------------------------------------------- linked worktree

git worktree add -q .claude/worktrees/w
cd .claude/worktrees/w || exit 1
run workflow hygiene --would-create CLAUDE.md
is "$RC" 1 'worktree: a new CLAUDE.md exits 1'
is "$OUT" 'CLAUDE.md:0: hard agent file: this belongs in mem' 'worktree: named from its own top'
cd "$app" || exit 1
run workflow hygiene --would-create .claude/worktrees/w/CLAUDE.md
is "$RC" 1 'worktree: asked from the main checkout, judged in its own tree'

## ------------------------------------------------------------ unregistered

new_repo stray
run workflow hygiene --would-create CLAUDE.md
is "$RC:$OUT" '0:' 'unregistered: a new CLAUDE.md passes'
printf 'seed\n' >CLAUDE.md
git add -f CLAUDE.md
run workflow hygiene --staged --known
is "$RC:$OUT" '0:' 'unregistered: --staged --known reads nothing'
run workflow hygiene --staged
is "$RC" 1 'unregistered: --staged alone still reads'

cd "$T_TMP" || exit 1
run workflow hygiene --would-create "$T_TMP/loose/CLAUDE.md"
is "$RC:$OUT" '0:' 'no work tree: passes'

## ------------------------------------------------------------------ usage

run workflow hygiene --known
is "$RC" 2 'usage: --known wants --staged'
