#!/usr/bin/env bash
# Strikes and owner findings. strike's walk is skipped twice, each walked
# again on the next tick, and fails the third time: the project pauses with
# one human question, no findings lead, and the answer `walk again` resumes
# it into a fix run and a fourth walk. owner's findings lead asks the owner
# about its one finding, and the milestone ticks with the finding open.
# block's finding waits on a blocking question when its walk is read: the
# milestone holds `waiting` with no strike and no lead until the answer,
# which reaches the findings lead.
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"
unset WORKFLOW_STATUS_FILE
mkdir -p "$XDG_CONFIG_HOME/qshell"
printf 'here\n' >"$XDG_CONFIG_HOME/qshell/machine"

# The fake session: every start is logged by what it is and whose. strike's
# walks 1 and 2 report blocked, walk 3 files a finding and fails, walk 4
# passes. owner's walk files a finding and fails; block's does the same with
# the blocking question already asked, as an earlier lead would have. The
# findings lead adds a fix task for strike, asks the owner for owner, and
# keeps its brief. Any other lead does nothing; a task commits its file.
write_exec "$T_TMP/fake-worker.sh" <<'FAKE'
#!/bin/sh
task=$1; status=$3; brief=$4
done_json() { printf '{"is_error":false,"result":"ok"}\n'; }
finding() {
	mem finding add --milestone m1 --step 2 'the heading reads Helo' >/dev/null
	mem finding list --open --json | jq -r '.items[0].short_id'
}
goal=$(sed -n '/^## GOAL/,/^## SCOPE/p' "$brief")
case "$goal" in
*'Walk the Show path'*)
	n=$(($(grep -c "^dogfood $MEM_PROJECT " "$WF_TMP/sessions.log" 2>/dev/null) + 1))
	printf 'dogfood %s %s\n' "$MEM_PROJECT" "$n" >>"$WF_TMP/sessions.log"
	case "$MEM_PROJECT $n" in
	'strike 1' | 'strike 2')
		workflow report blocked 'the dev server never came up'
		;;
	'strike 3' | 'owner 1')
		finding >/dev/null
		workflow report ready 'failed 2'
		;;
	'block 1')
		id=$(finding)
		mem ask --for human "blocking finding #$id: keep the old heading?" \
			--options 'keep,change' --recommend change >/dev/null
		workflow report ready 'failed 2'
		;;
	*)
		workflow report ready pass
		;;
	esac
	done_json
	exit 0
	;;
*'Turn the open findings into fix tasks'*)
	printf 'lead findings %s\n' "$MEM_PROJECT" >>"$WF_TMP/sessions.log"
	cp "$brief" "$WF_TMP/findings-$MEM_PROJECT.md"
	case "$MEM_PROJECT" in
	strike)
		printf -- '- [ ] fix-1 Spell the heading Hello\n      Files: app/fix-1.php\n      Verify: true\n' |
			mem plan --add-task >/dev/null
		;;
	owner)
		id=$(mem finding list --open --json | jq -r '.items[0].short_id')
		mem ask --for human "finding #$id: is Helo the brand's spelling?" \
			--options 'yes,no' --recommend yes >/dev/null
		;;
	esac
	done_json
	exit 0
	;;
*'of the lead skill'*)
	printf 'lead other %s\n' "$MEM_PROJECT" >>"$WF_TMP/sessions.log"
	done_json
	exit 0
	;;
esac
printf 'task %s\n' "$task" >>"$WF_TMP/sessions.log"
say() { printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" >>"$status"; }
say started
mkdir -p app
printf '%s\n' "$task" >"app/$task.php"
git add "app/$task.php"
git -c core.hooksPath=/dev/null commit -qm "Add a service"
say ready
done_json
FAKE
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$WF_TMP/fake-worker.sh" {task} {worktree} {status} {brief}'"'"' > {out} 2> {err} &'

# project <name>: a checkout whose one milestone has a two-step Show line and
# a plan of one task.
project() {
	new_repo "$1"
	mem_register
	"$MEM_BIN" project set verify true >/dev/null
	printf '# roadmap: %s-road\n\n- [ ] m1 The home page\n      Surface: web\n      Show: open the home page, the heading reads Hello\n' "$1" |
		"$MEM_BIN" roadmap --stdin >/dev/null
	"$MEM_BIN" roadmap --status approved >/dev/null
	"$MEM_BIN" plan m1 --stdin >/dev/null <<-EOF
	# plan: m1

	- [ ] a1 Add the a1 service
	      Files: app/a1.php
	      Verify: true
	EOF
}

project strike
project owner
project block

serve_dir() { printf '%s/workflow/serve/%s' "$XDG_STATE_HOME" "$1"; }
runs() { "$MEM_BIN" --project "$1" log --type run 2>/dev/null; }
walks() { runs "$1" | grep -o 'dogfood m1: .*' | tac | sed 's/^dogfood m1: //' | paste -sd ';'; }
stage() { cat "$(serve_dir "$1")/stage" 2>/dev/null; }
human() { "$MEM_BIN" --project "$1" questions --for human --json | jq -r '.questions[] | select(.answer == null) | .body'; }
road() { "$MEM_BIN" --project "$1" roadmap; }
leads() { grep -c "^lead findings $1$" "$T_TMP/sessions.log" 2>/dev/null; }
until_() { for _ in $(seq "$1"); do eval "$2" && return 0; sleep 1; done; return 1; }

cd "$T_TMP" || exit 1
WORKFLOW_DEADLINE_MIN=0.5 workflow serve --tick 1 >"$T_TMP/serve.out" 2>&1 &
serve_pid=$!
trap 'st=$?; kill "$serve_pid" 2>/dev/null; wait "$serve_pid" 2>/dev/null; (exit $st); t_done' EXIT

## ------------------------------------------------------- the third strike

S=$(serve_dir strike)
until_ 180 '[ "$(stage strike)" = paused ]' || notok 'three failed walks pause the project' "$(tail -15 "$T_TMP/serve.out")"
is "$(stage strike)" paused 'the third failed walk leaves the stage paused'
is "$(walks strike)" 'skipped the session reported blocked;skipped the session reported blocked;findings 1' 'two skipped walks, each walked again, then a failed one'
like "$(head -1 "$S/walk" 2>/dev/null)" '^m1 3 ' 'the walk file holds walk 3'
is "$(sed -n 3p "$S/walk" 2>/dev/null)" 'strikes 3' 'with three strikes'
like "$("$MEM_BIN" --project strike project current --json | jq -r '.paused')" '^here [0-9]{4}-[0-9]{2}-[0-9]{2}$' 'paused by this machine with the date'
asked=$(human strike)
is "$(printf '%s\n' "$asked" | grep -c .)" 1 'one human question'
like "$asked" '^m1 failed its Show path three times: #[0-9A-Z]+ open m1 2 the heading reads Helo$' 'naming the open finding'
sleep 3
is "$(human strike | grep -c .)" 1 'asked once'
is "$(grep -c '^strikes m1$' "$S/served")" 1 'under the served key strikes m1'
is "$(leads strike)" 0 'and no findings lead goes while paused'

## ------------------------------------------------------------ walk again

id=$("$MEM_BIN" --project strike questions --for human --json | jq -r '.questions[] | select(.body | startswith("m1 failed")) | .short_id')
"$MEM_BIN" --project strike answer "$id" --option 'walk again' >/dev/null
until_ 120 'grep -q "^dogfood strike 4$" "$T_TMP/sessions.log"' || notok 'walk again starts a fourth walk' "$(tail -15 "$T_TMP/serve.out")"
grep -q '^dogfood strike 4$' "$T_TMP/sessions.log" && ok 'walk again starts a fourth walk'
is "$("$MEM_BIN" --project strike project current --json | jq -r '.paused')" null 'and unpauses the project'
is "$(leads strike)" 1 'the failed third walk goes to its findings lead'
until_ 60 'road strike | grep -q "^- \[x\] m1 "'
like "$(road strike)" '^- \[x\] m1 ' 'the fourth walk passes and ticks the milestone'
[ -e "$S/walk" ] && notok 'the walk file goes with the pass' "$(cat "$S/walk")" || ok 'the walk file goes with the pass'

## -------------------------------------------------- a non-blocking owner finding

until_ 60 'road owner | grep -q "^- \[x\] m1 "'
like "$(road owner)" '^- \[x\] m1 ' 'a finding asked of the owner lets the milestone tick'
is "$(walks owner)" 'findings 1;pass' 'the failed walk, then the pass the question makes of it'
is "$("$MEM_BIN" --project owner finding list --open --json | jq '.items | length')" 1 'with the finding open'
is "$(leads owner)" 1 'after the one findings lead that asked'

## ------------------------------------------------------ a blocking one

B=$(serve_dir block)
until_ 60 'runs block | grep -q "dogfood m1:"' || notok 'the blocked walk is read' "$(tail -15 "$T_TMP/serve.out")"
sleep 3
is "$(stage block)" waiting 'a pending blocking finding holds waiting'
is "$(sed -n 2p "$B/walk" 2>/dev/null)" 'failed 2' 'the walk file keeps the outcome'
is "$(sed -n 3p "$B/walk" 2>/dev/null)" '' 'with no strike'
is "$(leads block)" 0 'and no findings lead'
like "$(road block)" '^- \[ \] m1 ' 'the milestone stays open'
id=$("$MEM_BIN" --project block questions --for human --json | jq -r '.questions[] | select(.body | startswith("blocking finding")) | .short_id')
"$MEM_BIN" --project block answer "$id" 'change it to Hello' >/dev/null
until_ 60 '[ "$(leads block)" = 1 ]'
is "$(leads block)" 1 'the answer starts the findings lead'
like "$(cat "$T_TMP/findings-block.md" 2>/dev/null)" 'change it to Hello' 'with the answer in its brief'
