#!/usr/bin/env bash
# A failed walk becomes fix tasks through a lead: serve starts one findings
# lead over the milestone's open findings, the lead appends a fix task to the
# plan of record, the run lands it with no second pickup, and the walk after
# it covers only the step the finding stands on. That walk passing closes the
# finding as fixed by the trunk head and ticks the milestone.
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"
unset WORKFLOW_STATUS_FILE
mkdir -p "$XDG_CONFIG_HOME/qshell"
printf 'here\n' >"$XDG_CONFIG_HOME/qshell/machine"

# The fake session: every start is logged by what it is. The first walk files
# a finding on step 2 with the page as its capture and reports it failed; the
# second keeps its brief, waits to be let go and passes. The findings lead
# appends fix-1; any other lead does nothing. A task commits its file.
write_exec "$T_TMP/fake-worker.sh" <<'FAKE'
#!/bin/sh
task=$1; status=$3; brief=$4
done_json() { printf '{"is_error":false,"result":"ok"}\n'; }
goal=$(sed -n '/^## GOAL/,/^## SCOPE/p' "$brief")
case "$goal" in
*'Walk the Show path'*)
	n=$(($(grep -c '^dogfood' "$WF_TMP/sessions.log" 2>/dev/null) + 1))
	printf 'dogfood %s\n' "$n" >>"$WF_TMP/sessions.log"
	cp "$brief" "$WF_TMP/walk$n.md"
	if [ "$n" = 1 ]; then
		printf '<h1>Helo</h1>\n' >"$WF_TMP/page.html"
		mem finding add --milestone m1 --step 2 --evidence "$WF_TMP/page.html" \
			'the heading reads Helo' >/dev/null
		workflow report ready 'failed 2'
	else
		for _ in $(seq 600); do [ -e "$WF_TMP/release" ] && break; sleep 0.2; done
		workflow report ready pass
	fi
	done_json
	exit 0
	;;
*'Turn the open findings into fix tasks'*)
	printf 'lead findings\n' >>"$WF_TMP/sessions.log"
	cp "$brief" "$WF_TMP/findings.md"
	printf -- '- [ ] fix-1 Spell the heading Hello\n      Files: app/fix-1.php\n      Verify: true\n' |
		mem plan --add-task >/dev/null
	done_json
	exit 0
	;;
*'of the lead skill'*)
	printf 'lead other\n' >>"$WF_TMP/sessions.log"
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

new_repo app
mem_register
"$MEM_BIN" project set verify true >/dev/null
printf '# roadmap: app-road\n\n- [ ] m1 The home page\n      Surface: web\n      Show: open the home page, the heading reads Hello, the footer shows the year\n' |
	"$MEM_BIN" roadmap --stdin >/dev/null
"$MEM_BIN" roadmap --status approved >/dev/null
"$MEM_BIN" plan m1 --stdin >/dev/null <<-EOF
# plan: m1

- [ ] a1 Add the a1 service
      Files: app/a1.php
      Verify: true
EOF

S="$XDG_STATE_HOME/workflow/serve/app"
runs() { "$MEM_BIN" --project app log --type run 2>/dev/null; }
walk_lines() { runs | grep -o 'dogfood m1: .*'; }

cd "$T_TMP" || exit 1
WORKFLOW_DEADLINE_MIN=0.5 workflow serve --tick 1 >"$T_TMP/serve.out" 2>&1 &
serve_pid=$!
trap 'st=$?; touch "$T_TMP/release"; kill "$serve_pid" 2>/dev/null; wait "$serve_pid" 2>/dev/null; (exit $st); t_done' EXIT

for _ in $(seq 180); do [ -e "$T_TMP/walk2.md" ] && break; sleep 1; done
[ -e "$T_TMP/walk2.md" ] || notok 'the fixed milestone is walked again' "$(tail -15 "$T_TMP/serve.out")"

## ------------------------------------------------------- the findings lead

is "$(grep -c '^lead findings$' "$T_TMP/sessions.log" 2>/dev/null)" 1 'one findings lead for the failed walk'
like "$(cat "$T_TMP/findings.md" 2>/dev/null)" 'open +m1 +2 +the heading reads Helo' 'its brief lists the open finding'
is "$(grep -c '^findings m1 1$' "$S/served" 2>/dev/null)" 1 'served once for walk 1'
is "$(grep -c '^lead other$' "$T_TMP/sessions.log" 2>/dev/null)" 1 'the fix run starts with no second pickup'
is "$(grep -c '^task fix-1$' "$T_TMP/sessions.log" 2>/dev/null)" 1 'the fix task runs'
like "$("$MEM_BIN" --project app plan)" '\- \[x\] fix-1 ' 'and lands'

## --------------------------------------------------------- the second walk

w2=$(sed -n '/^## GOAL/,/^## SCOPE/p' "$T_TMP/walk2.md" 2>/dev/null)
like "$w2" '2\. the heading reads Hello' 'the second walk names step 2'
unlike "$w2" '1\. open the home page' 'and not step 1'
unlike "$w2" '3\. the footer shows the year' 'nor step 3'
like "$(cat "$T_TMP/walk2.md" 2>/dev/null)" 'the heading reads Helo' 'its brief lists the finding'
like "$(cat "$S/walk" 2>/dev/null)" '^m1 2 [0-9]+ [^ ]+ 2$' 'the walk file holds walk 2 of step 2'
like "$("$MEM_BIN" --project app roadmap)" '^- \[ \] m1 ' 'the milestone stays open through the second walk'
is "$(walk_lines | grep -c .)" 1 'only the first walk is read so far'

## ---------------------------------------------------------------- the pass

touch "$T_TMP/release"
for _ in $(seq 60); do "$MEM_BIN" --project app roadmap | grep -q '^- \[x\] m1 ' && break; sleep 1; done
like "$("$MEM_BIN" --project app roadmap)" '^- \[x\] m1 ' 'the second walk ticks the milestone'
log=$(walk_lines)
like "$log" 'dogfood m1: findings 1' 'the run log holds the failed walk'
like "$log" 'dogfood m1: pass' 'and the pass'
# mem lists the newest first.
is "$(printf '%s\n' "$log" | tac | sed 's/^dogfood m1: //' | paste -sd ';')" 'findings 1;pass' 'findings 1, then pass, and no other walk'
found=$("$MEM_BIN" --project app finding list --json)
like "$found" '"status":"fixed"' 'the finding is closed'
like "$found" "\"fixed_by\":\"$(git -C "$T_TMP/app" rev-parse HEAD)\"" 'as fixed by the trunk head'
is "$(grep -c '^lead findings$' "$T_TMP/sessions.log")" 1 'and exactly one findings lead ran'
[ -e "$S/walk" ] && notok 'the walk file goes with the pass' "$(cat "$S/walk")" || ok 'the walk file goes with the pass'
