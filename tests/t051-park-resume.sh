#!/usr/bin/env bash
# park and resume (spec §8.9, AC9): the bundle carries base_sha..branch
# including the wip commit, it lands outside mem's store, resume puts the
# uncommitted state back, and retention keeps five per project.
source "$(dirname -- "$0")/lib.sh"
t_init

new_repo work
mem_register
base=$(git rev-parse HEAD)
git checkout -qb feature/cart
printf 'committed\n' >committed.txt
git add committed.txt
git -c core.hooksPath=/dev/null commit -qm 'Add the committed part'
printf 'half written\n' >half.txt
printf 'edited\n' >>README.md
git add README.md

run park --repo "$PWD" --branch feature/cart --base "$base" --note 'stopped for the day'
is "$RC" 0 'park exits clean'
bundle=$(printf '%s\n' "$OUT" | grep '\.bundle$' | head -1)
like "$bundle" '\.bundle$' 'park prints the bundle path'
truthy "$([ -f "$bundle" ] && echo 0 || echo 1)" 'and the bundle is there'
like "$OUT" 'wip' 'park says it saved the uncommitted work as a wip commit'

## ------------------------------------------------- what the bundle carries

is "$(git log -1 --format=%s)" 'wip: parked work, restore with resume' 'the tip is the wip commit'
run git bundle verify "$bundle"
is "$RC" 0 'the bundle verifies against this repo'
is "$(git bundle list-heads "$bundle" | grep -c 'refs/heads/feature/cart')" 1 'it carries the branch'
is "$(git rev-list --count "$base..feature/cart")" 2 'base_sha..branch is the committed change plus the wip commit'

## --------------------------------------------------- and where it lands

like "$bundle" "$XDG_DATA_HOME/workflow/parked" 'the bundle lands in the parked directory'
unlike "$bundle" 'mem/store' 'which is nowhere near mem'
is "$(find "$XDG_DATA_HOME/mem/store" -name '*.bundle' 2>/dev/null | grep -c .)" 0 "mem's store has no strays in it"
run "$MEM_BIN" doctor
is "$RC" 0 'mem doctor is happy'
unlike "$OUT" 'workflow' 'and has nothing to say about the parked bundles'

## ------------------------------------------------------------- and back

new_repo elsewhere
git fetch -q "$T_TMP/work" 'refs/heads/main:refs/heads/mainline' 2>/dev/null
cd "$T_TMP/work" || exit 1
git checkout -q main
git branch -D feature/cart >/dev/null
rm -f half.txt committed.txt
git checkout -q -- README.md

run resume "$bundle"
is "$RC" 0 'resume exits clean'
is "$(git rev-parse --abbrev-ref HEAD)" 'feature/cart' 'the branch is back and checked out'
is "$(git log -1 --format=%s)" 'Add the committed part' 'the wip commit is unwound, the real one is the tip'
is "$(cat half.txt 2>/dev/null)" 'half written' 'the untracked half-written file is back'
like "$(git status --porcelain -uall)" 'M  README.md' 'and the staged edit is staged again'
like "$(git status --porcelain -uall)" 'A  half.txt' 'reset --soft leaves the wip content in the index'
# It all comes back staged, including what was not staged when park ran: the
# wip commit does not record the split, so resume cannot restore it. The help
# text says so rather than claiming otherwise.
like "$(resume --help)" 'staged/unstaged split' 'resume says what it does not restore'

## ------------------------------------------------------------- retention

for i in 1 2 3 4 5 6 7; do
	git checkout -q main
	git branch -D "throw-$i" >/dev/null 2>&1
	git checkout -qb "throw-$i"
	printf '%s\n' "$i" >"file-$i"
	git add "file-$i"
	git -c core.hooksPath=/dev/null commit -qm "Add file $i"
	sleep 1.05 # the bundle name carries a whole-second timestamp
	park --repo "$PWD" --branch "throw-$i" --base "$base" --name "throw-$i" >/dev/null 2>&1
done
kept=$(find "$XDG_DATA_HOME/workflow/parked" -name '*.bundle' | grep -c .)
is "$kept" 5 'retention keeps five bundles per project'
truthy "$([ -f "$(find "$XDG_DATA_HOME/workflow/parked" -name 'throw-7-*.bundle' | head -1)" ] && echo 0 || echo 1)" \
	'and the newest is one of them'
is "$(find "$XDG_DATA_HOME/workflow/parked" -name 'throw-1-*.bundle' | grep -c .)" 0 'the oldest is gone'
is "$(find "$XDG_DATA_HOME/workflow/parked" -name 'throw-1-*.meta' | grep -c .)" 0 'and so is its meta file'

## ------------------------------------------------------- nothing to park

git checkout -q main
git branch -D empty-branch >/dev/null 2>&1
git checkout -qb empty-branch
run park --repo "$PWD" --branch empty-branch --base "$base" --name empty
is "$RC" 0 'a branch with no work on it is not an error'
like "$OUT" 'nothing to park' 'park just says there is nothing there'

## ----------------------------------- park found from a one-command install

# An install that symlinks `workflow` onto PATH does not put `park` beside it.
# The helper is resolved from where the real script lives, so an orchestrated
# park still bundles.
mkdir -p "$T_TMP/onlybin"
ln -sf "$WORKFLOW_BIN" "$T_TMP/onlybin/workflow"
is "$([ -e "$T_TMP/onlybin/park" ] && printf yes || printf no)" no \
	'the one-command install really has no park next to it'

export WF_TMP="$T_TMP"
write_exec "$T_TMP/lonely-worker.sh" <<'FAKE'
#!/bin/sh
task=$1; status=$3
printf '%s started\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$status"
printf 'outside\n' >NOTOWNED.txt
git add NOTOWNED.txt
git -c core.hooksPath=/dev/null commit -qm "Reach outside the task"
printf '%s ready\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$status"
printf '{"is_error":false,"result":"ok"}\n'
FAKE
export FAKE="$T_TMP/lonely-worker.sh"
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$FAKE" {task} {worktree} {status}'"'"' > {out} 2> {err} &'

new_repo lonely
mem_register
printf '{"name":"acme/l"}\n' >composer.json
printf '#!/bin/sh\nexit 0\n' >artisan
chmod +x artisan
write_exec bin/php <<-'PHP'
	#!/bin/sh
	exit 0
PHP
git add -A
git -c core.hooksPath=/dev/null commit -qm 'project files'
cat >"$T_TMP/lonely-plan.md" <<'PLAN'
# plan: lonely

- [ ] t1 Reach outside
      Files: app/One.php
      Verify: true
- [ ] t2 Reach outside as well
      Files: app/Two.php
      Verify: true
PLAN
run env WORKFLOW_MAX_WORKERS=2 WORKFLOW_DEADLINE_MIN=0.5 \
	"$T_TMP/onlybin/workflow" run --plan-file "$T_TMP/lonely-plan.md"
is "$RC" 1 'the lonely-install run parks both tasks'
unlike "$OUT" 'could not bundle' 'and park was found, so nothing failed to bundle'
is "$(find "$XDG_DATA_HOME/workflow/parked/lonely" -name 'lonely-t*.bundle' 2>/dev/null | grep -c .)" 2 \
	'both parked tasks left a bundle behind'

## ------------------------------------------------ park asks for a sync round

# park ends by firing the parked unit, detached, so the bundle reaches the
# other machine now rather than on the 15-minute timer (friction #KAPRWGBB).
write_exec "$T_TMP/fake-sync" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >>"$SYNC_LOG"
EOF
export SYNC_LOG="$T_TMP/sync.log"
export WORKFLOW_SYNC_CMD="$T_TMP/fake-sync"

cd "$T_TMP/work" || exit 1
git checkout -q main
git checkout -qb sync-me
printf 'sync\n' >sync.txt
git add sync.txt
git -c core.hooksPath=/dev/null commit -qm 'Add the synced part'
run park --repo "$PWD" --branch sync-me --base "$base" --name sync-me
is "$RC" 0 'park with a sync command exits clean'
bundle=$(printf '%s\n' "$OUT" | grep '^/.*\.bundle$' | head -1)
for _ in 1 2 3 4 5 6 7 8 9 10; do
	grep -q 'only parked' "$SYNC_LOG" 2>/dev/null && break
	sleep 0.2
done
like "$(cat "$SYNC_LOG" 2>/dev/null)" 'only parked' 'park fired the parked unit'

## --------------------------------------------- resume pulls before giving up

# A bundle parked on the other machine is not here yet: resume asks for a
# round, waits it out, and looks again.
hidden="$T_TMP/hidden"
mkdir -p "$hidden"
mv "$bundle" "$hidden/"
mv "${bundle%.bundle}.meta" "$hidden/"
write_exec "$T_TMP/delivering-sync" <<EOF
#!/bin/sh
printf '%s\n' "\$*" >>"\$SYNC_LOG"
cp "$hidden"/* "$(dirname -- "$bundle")/"
EOF
export WORKFLOW_SYNC_CMD="$T_TMP/delivering-sync"
git checkout -q main
git branch -D sync-me >/dev/null
rm -f sync.txt
run resume "$bundle"
is "$RC" 0 'resume finds the bundle a sync round delivered'
is "$(git rev-parse --abbrev-ref HEAD)" 'sync-me' 'and the branch is back'

# When the round delivers nothing, resume says what it tried.
export WORKFLOW_SYNC_CMD="$T_TMP/fake-sync"
run resume "$XDG_DATA_HOME/workflow/parked/work/never-there.bundle"
is "$RC" 1 'a bundle no round delivers is an error'
like "$OUT" 'sync' 'and resume says it asked for a round'
