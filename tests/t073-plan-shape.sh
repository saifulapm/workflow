#!/usr/bin/env bash
# plan-check's shape warnings (ruling 3 of m1-wiki-first): a plan with no
# prose above its tasks, a task whose Files carries more than eight
# patterns, a Done sentence over forty words, and a Read: naming a wiki page
# the project has no such page -- four warnings, never a refusal. A wiki:
# item never trips the existing "not here to be read" warning, and a
# roadmap's milestone plan earns every one of them exactly as a plan of its
# own would. A Read or Pattern path on disk that git does not track warns
# too: no worktree has it.
source "$(dirname -- "$0")/lib.sh"
t_init

new_repo shape
mem_register

plan() { cat >"plan.md"; }
check() {
	OUT=$(workflow plan-check plan.md 2>"$T_TMP/check.err")
	RC=$?
	ERR=$(cat "$T_TMP/check.err")
}

## ------------------------------------------------------- no prose at all

plan <<'EOF'
# plan: shape

- [ ] t1 Do the thing
      Files: a.rs
      Verify: true
EOF
check
is "$RC" 0 'warnings do not refuse the plan'
like "$ERR" \
	"plan: no prose above the tasks -- the worker and the reader see only the task blocks; write the Spec and the Rulings first" \
	'a plan with nothing above its tasks warns'

## ----------------------------------------------------------- with prose

plan <<'EOF'
# plan: shape

## Spec

Do the thing.

## Rulings

1. Do it well.

- [ ] t1 Do the thing
      Files: a.rs
      Verify: true
EOF
check
unlike "$ERR" 'no prose above the tasks' 'prose above the tasks clears the warning'

## ------------------------------------------------------------- Files width

plan <<'EOF'
# plan: shape

## Spec

Do the thing.

## Rulings

1. Do it well.

- [ ] t1 Do the thing
      Files: a.rs b.rs c.rs d.rs e.rs f.rs g.rs h.rs i.rs
      Verify: true
EOF
check
is "$RC" 0 'warnings do not refuse the plan'
like "$ERR" "plan: task t1: Files carries 9 patterns -- a task owning more than eight is two tasks" \
	'a task with nine Files patterns warns'

plan <<'EOF'
# plan: shape

## Spec

Do the thing.

## Rulings

1. Do it well.

- [ ] t1 Do the thing
      Files: a.rs b.rs c.rs d.rs e.rs f.rs g.rs h.rs
      Verify: true
EOF
check
unlike "$ERR" 'Files carries' 'eight patterns is still one task'

## -------------------------------------------------------------- Done length

done_long=$(printf 'word %.0s' $(seq 1 41))
plan <<EOF
# plan: shape

## Spec

Do the thing.

## Rulings

1. Do it well.

- [ ] t1 Do the thing
      Files: a.rs
      Verify: true
      Done: $done_long
EOF
check
like "$ERR" "plan: task t1: Done is 41 words -- a sentence over forty is a fix verdict waiting; split the task or the sentence" \
	'a Done sentence over forty words warns'

done_ok=$(printf 'word %.0s' $(seq 1 40))
plan <<EOF
# plan: shape

## Spec

Do the thing.

## Rulings

1. Do it well.

- [ ] t1 Do the thing
      Files: a.rs
      Verify: true
      Done: $done_ok
EOF
check
unlike "$ERR" 'Done is' 'forty words is still one sentence'

## --------------------------------------------- a manifest without its lockfile

printf '{"name":"shape"}\n' >package.json
printf 'lockfileVersion: 9\n' >pnpm-lock.yaml
printf 'packages:\n' >pnpm-workspace.yaml
printf '[package]\nname = "shape"\n' >Cargo.toml
git add package.json pnpm-lock.yaml pnpm-workspace.yaml Cargo.toml
git -c core.hooksPath=/dev/null commit -qm 'manifests'
plan <<'EOF'
# plan: shape

## Spec

Do the thing.

## Rulings

1. Do it well.

- [ ] deps Add a dependency
      Files: apps/x/package.json apps/x/src/**
      Verify: true
- [ ] rust Add a crate
      Files: Cargo.toml src/lib.rs
      Verify: true
- [ ] both Add a dependency and claim the lock
      Files: package.json pnpm-lock.yaml pnpm-workspace.yaml
      Verify: true
- [ ] none Touch no manifest
      Files: src/other.rs
      Verify: true
EOF
check
is "$RC" 0 'a manifest without its lockfile warns, never refuses'
like "$ERR" "plan: task deps: Files claims package.json and not pnpm-lock.yaml and pnpm-workspace.yaml -- installing rewrites it and the gate refuses whatever a task writes outside Files \(add pnpm-lock.yaml and pnpm-workspace.yaml here and an \[after:\] to serialize it against the other lock writers\)" \
	'naming the lockfiles the tree has for package.json, the workspace file too'
like "$ERR" "plan: task rust: Files claims Cargo.toml and not Cargo.lock" 'and the cargo lockfile for Cargo.toml, tracked or not'
unlike "$ERR" 'task both: Files claims' 'a task claiming the lockfiles is not warned'
unlike "$ERR" 'task none: Files claims' 'nor one claiming no manifest'
like "$ERR" "plan: task deps: 2 patterns match nothing here and their directories do not exist -- tasks creating them, or typos: 'apps/x/package.json', 'apps/x/src/\*\*'" \
	'the paths a task will create are one line, naming them'
like "$ERR" "plan: task none: 'src/other.rs' matches nothing here and its directory does not exist -- a task creating it, or a typo" \
	'and one path keeps the one-path wording'
like "$OUT" '^  widest wave: 4 tasks$' 'the listing ends with the widest wave'

## ------------------------------------------------------------ wiki pages

printf 'The run drives waves.\n' | "$MEM_BIN" wiki run --stdin --note seed >/dev/null

plan <<'EOF'
# plan: shape

## Spec

Do the thing.

## Rulings

1. Do it well.

- [ ] t1 Do the thing
      Files: a.rs
      Read: wiki:run wiki:missing
      Verify: true
EOF
check
is "$RC" 0 'warnings do not refuse the plan'
like "$ERR" "plan: task t1: Read names wiki:missing and the project has no such page" \
	'a wiki: page the project does not have warns'
unlike "$ERR" 'wiki:run and the project has no such page' \
	'a wiki: page the project does have does not warn'
unlike "$ERR" "Read names 'wiki:run' and it is not here to be read" \
	'a wiki: item never trips the "not here to be read" warning'
unlike "$ERR" "Read names 'wiki:missing' and it is not here to be read" \
	'not even an absent one'

## ------------------------------------------ an untracked Read or Pattern

# A file on disk that git does not track is in no worktree, so the worker
# the Read names it to never sees it (friction #NJQXGXGE).
mkdir -p notes
printf 'review\n' >notes/review.md
printf 'style\n' >notes/style.md
plan <<'EOF'
# plan: shape

## Spec

Do the thing.

## Rulings

1. Do it well.

- [ ] t1 Do the thing
      Files: a.rs
      Read: notes/review.md README.md
      Pattern: notes/style.md:1-2
      Verify: true
EOF
check
is "$RC" 0 'warnings do not refuse the plan'
like "$ERR" "plan: task t1: Read names 'notes/review.md' and git does not track it -- no worktree will have it" \
	'an untracked Read path warns'
like "$ERR" "plan: task t1: Pattern points at 'notes/style.md' and git does not track it -- no worktree will have it" \
	'an untracked Pattern path warns'
unlike "$ERR" "'README.md' and git does not track it" 'a tracked Read path does not'
unlike "$ERR" "'notes/review.md' and it is not here to be read" \
	'an untracked file on disk is not called absent'
rm -r notes

## ------------------------------------------ a milestone plan in a roadmap

# A milestone plan is judged exactly as a plan checked on its own would be:
# ruling 3 names no exemption for it, and poshra's 36 prose-less plans were
# a roadmap's milestones.
mkdir -p road
cat >road/roadmap.md <<'EOF'
# roadmap: shop

- [ ] m1 First
EOF
cat >road/m1.md <<'EOF'
# plan: m1

- [ ] t1 Do the thing
      Files: a.rs
      Verify: true
EOF
OUT=$(workflow plan-check road/roadmap.md 2>"$T_TMP/road.err")
RC=$?
ROAD_ERR=$(cat "$T_TMP/road.err")
is "$RC" 0 'the roadmap is not refused'
like "$ROAD_ERR" 'm1: plan: no prose above the tasks' \
	"a milestone plan with no prose of its own earns the warning too"
