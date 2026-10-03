#!/usr/bin/env bash
# workflow hygiene: the hard tier on a staged diff, a message, the tree and
# history; the soft tier on messages only; the id rule against CSS colors; the
# hygiene-exempt key; --json; and --fix untracking what the ignore list names.
source "$(dirname -- "$0")/lib.sh"
t_init

new_repo app
mem_register

# hit <label> <path:line> -- the finding line is in OUT with its tier.
hit() { like "$OUT" "^$2: hard $1: " "hard $1 at $2"; }

## ------------------------------------------------------------ staged diff

mkdir -p src docs
cat >src/app.rs <<'EOF'
fn main() {}
// cached because ruling 4 says so
// shipped in milestone 2
// see (issue 381)
// ticket 7 asked for it
// ADR 0012 covers this
// an owner decision
// done per the plan
// the plan of record
// see #QZ7RKWVM
// Co-Authored-By: someone
// Generated with a tool
// https://claude.ai/code/x
// 🤖
EOF
printf 'per ruling 4\n' >docs/notes.md
printf 'seed\n' >CLAUDE.md
mkdir -p .claude && printf '{}\n' >.claude/settings.json
printf 'node_modules\n/.claude/\n' >.gitignore
git add -f src docs CLAUDE.md .claude .gitignore

run workflow hygiene --staged
is "$RC" 1 'staged: a hard finding exits 1'
hit 'numbered ruling' 'src/app.rs:2'
hit 'numbered milestone' 'src/app.rs:3'
hit 'issue number' 'src/app.rs:4'
hit 'numbered ticket' 'src/app.rs:5'
hit 'numbered ADR' 'src/app.rs:6'
hit 'owner decision' 'src/app.rs:7'
hit 'per the plan' 'src/app.rs:8'
hit 'plan of record' 'src/app.rs:9'
hit 'memory id' 'src/app.rs:10'
hit 'co-author trailer' 'src/app.rs:11'
hit 'generated-with line' 'src/app.rs:12'
hit 'Claude Code link' 'src/app.rs:13'
hit 'robot emoji' 'src/app.rs:14'
hit 'ignored path' 'CLAUDE.md:0'
hit 'ignored path' '\.claude/settings\.json:0'
hit 'agent ignore line' '\.gitignore:2'
unlike "$OUT" 'docs/notes\.md' 'staged: markdown lines are not read'
unlike "$OUT" 'src/app\.rs:1:' 'staged: an ordinary line is not named'

run_out workflow hygiene --staged --json
is "$(jq -r '[.[] | select(.path == "src/app.rs")] | length' <<<"$OUT")" 13 '--json: one object per finding'
is "$(jq -r '.[0] | [.tier, (.line | type)] | join(" ")' <<<"$OUT")" 'hard number' '--json: tier and a numeric line'

git reset -q

## ------------------------------------------------------- ids and colours

cat >src/style.rs <<'EOF'
const A: &str = "#AABBCCDD";
const B: &str = "#ABCDEF12";
const C: &str = "#ABCDEFGH";
const D: &str = "#W8XKPLRN";
EOF
git add src/style.rs
run workflow hygiene --staged
is "$RC" 1 'ids: a mem id is hard'
hit 'memory id' 'src/style.rs:4'
unlike "$OUT" 'src/style\.rs:[123]:' 'ids: CSS colours and a digitless id pass'
git reset -q

## ---------------------------------------------------------- exemption key

mkdir -p tests
printf '# per ruling 4\n' >tests/fixture.sh
git add tests/fixture.sh
run workflow hygiene --staged
is "$RC" 1 'exempt: without the key the fixture is hard'
"$MEM_BIN" project set hygiene-exempt 'tests/**' >/dev/null
run workflow hygiene --staged
is "$RC" 0 'exempt: the key clears the paths it names'
unlike "$OUT" 'tests/fixture' 'exempt: and says nothing about them'
git reset -q

## ---------------------------------------------------------------- message

msg() { printf '%s\n' "$@" >"$T_TMP/msg"; run workflow hygiene --message "$T_TMP/msg"; }

msg 'Cache the price lookup' '' 'Ruling 4 asked for it.'
is "$RC" 1 'message: a numbered ruling is hard'
like "$OUT" "^$T_TMP/msg:3: hard numbered ruling: Ruling 4" 'message: named by file and line'
for text in 'milestone 2' '(issue 381)' 'ticket 7' 'ADR 0012' 'owner decision' 'per the plan' \
	'plan of record' '#QZ7RKWVM' 'Co-Authored-By: x' 'Generated with a tool' 'claude.ai/code' '🤖'; do
	run workflow hygiene --string "Cache the price lookup

Body naming $text here."
	is "$RC" 1 "message: $text is hard"
done
run workflow hygiene --string 'Cache the price lookup so the checkout page stops timing out under heavy load'
is "$RC" 1 'message: a subject over 72 characters is hard'
like "$OUT" ':1: hard long subject: ' 'message: and says so'
run workflow hygiene --string 'Finish t3 before the release'
like "$OUT" ':1: hard task id: ' 'message: a task id is hard'
run workflow hygiene --string 'Close out m4-ui cleanly'
like "$OUT" ':1: hard milestone id: ' 'message: a milestone id is hard'
run workflow hygiene --string 'Rename the t3a column and the m4 board'
is "$RC" 0 'message: ids inside words pass'

run workflow hygiene --string 'Expire the session cookie sooner'
is "$RC" 0 'soft: a soft word exits 0'
like "$OUT" ':1: soft session: ' 'soft: and is named'
"$MEM_BIN" save --kind ruling --type lint-exception \
	'"session" is the product vocabulary here - cookies have sessions - cost if wrong: a leaked word' >/dev/null
run workflow hygiene --string 'Expire the session cookie sooner'
unlike "$OUT" 'soft session' 'soft: a lint-exception clears it'

## ------------------------------------------------------------------- tree

git add src/app.rs CLAUDE.md docs/notes.md
git -c core.hooksPath=/dev/null commit -qm 'Add the app'
run workflow hygiene --tree
is "$RC" 1 '--tree: tracked findings are hard'
hit 'numbered ruling' 'src/app.rs:2'
hit 'ignored path' 'CLAUDE.md:0'
run workflow hygiene --tree --path docs
is "$RC" 0 '--path: only that directory is read'

## ---------------------------------------------------------------- history

c() { printf 'x\n' >>docs/log.txt; git add docs/log.txt; git -c core.hooksPath=/dev/null commit -qm "$1"; git rev-parse --short HEAD; }
old=$(c 'Old change citing ruling 9')
one=$(c 'Fix the cart total')
two=$(c 'Fix the tax rate per ruling 3')
three=$(c 'Raise the timeout on the supplier client so slow responses stop failing early')
run workflow hygiene --history 3
is "$RC" 1 '--history: a hard message exits 1'
hit 'numbered ruling' "$two:1"
hit 'long subject' "$three:1"
unlike "$OUT" "^$one:" '--history: a clean message is not named'
unlike "$OUT" "^$old:" '--history: nothing past the count is read'

## -------------------------------------------------------------------- fix

git rm -q --cached src/app.rs
printf 'node_modules\n.claude\n**/agent-memory/\n' >.gitignore
git add .gitignore
git -c core.hooksPath=/dev/null commit -qm 'Drop the app'
run workflow hygiene --tree --fix
is "$RC" 0 '--fix: nothing hard is left in the tree'
is "$(git ls-files CLAUDE.md)" '' '--fix: CLAUDE.md is untracked'
is "$(cat CLAUDE.md)" 'seed' '--fix: and stays on disk'
is "$(cat .gitignore)" 'node_modules' '--fix: agent lines leave the .gitignore'

## ------------------------------------------------------------------ usage

run workflow hygiene --staged --tree
is "$RC" 2 'usage: two modes at once'
