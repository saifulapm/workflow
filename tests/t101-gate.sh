#!/usr/bin/env bash
# The commit hooks run hygiene: pre-commit reads the staged diff, commit-msg
# reads the message, and WORKFLOW_HYGIENE=skip clears a human commit and never
# an agent's.
source "$(dirname -- "$0")/lib.sh"
t_init
chmod +x "$HOOKS"/pre-commit "$HOOKS"/commit-msg 2>/dev/null

## ------------------------------------------- an agent in a registered checkout

new_repo agent
mem_register

mkdir -p src
printf 'fn main() {}\n// cached because ruling 4 says so\n' >src/cache.rs
git add src/cache.rs
run env WORKFLOW_AGENT=1 git -c core.hooksPath="$HOOKS" commit -m 'Cache the supplier lookup'
isnt "$RC" 0 'pre-commit: a line citing a numbered ruling is refused'
like "$OUT" '^src/cache\.rs:2: hard numbered ruling: // cached because ruling 4 says so' 'pre-commit: with its path and line'
is "$(git log --oneline | wc -l)" 1 'pre-commit: nothing was committed'

run env WORKFLOW_AGENT=1 WORKFLOW_HYGIENE=skip git -c core.hooksPath="$HOOKS" commit -m 'Cache the supplier lookup'
isnt "$RC" 0 'skip: ignored with WORKFLOW_AGENT set'
like "$OUT" 'hard numbered ruling' 'skip: and the finding still named'

printf 'fn main() {}\n// cached because the supplier allows twelve calls a minute\n' >src/cache.rs
git add src/cache.rs
subject='Cache the supplier lookup for five minutes so the importer stays under its limit'
is "${#subject}" 80 'the subject is 80 characters'
run env WORKFLOW_AGENT=1 git -c core.hooksPath="$HOOKS" commit -m "$subject"
isnt "$RC" 0 'commit-msg: an 80-character subject is refused'
like "$OUT" 'hard long subject: ' 'commit-msg: named as a long subject'

run env WORKFLOW_AGENT=1 git -c core.hooksPath="$HOOKS" commit -m 'Cache the supplier lookup'
is "$RC" 0 'the reason in place of the citation, under a short subject, goes through'

## ------------------------------------------- a human in a registered checkout

new_repo human
mem_register
printf 'fn main() {}\n// cached because ruling 4 says so\n' >cache.rs
git add cache.rs

run env -u WORKFLOW_AGENT -u PI_CODING_AGENT git -c core.hooksPath="$HOOKS" commit -m 'Cache the supplier lookup'
isnt "$RC" 0 'a human commit in a registered checkout is held to hygiene too'
like "$OUT" '^cache\.rs:2: hard numbered ruling: ' 'and told the line'

run env -u WORKFLOW_AGENT -u PI_CODING_AGENT WORKFLOW_HYGIENE=skip git -c core.hooksPath="$HOOKS" commit -m "$subject"
is "$RC" 0 'skip: honoured without WORKFLOW_AGENT, for the diff and the message'
is "$(git log -1 --format=%s)" "$subject" 'skip: the commit landed'
