#!/usr/bin/env bash
# In maintenance serve works the open findings one at a time, oldest first,
# each through a fix plan a lead stores under fix-<id>. A one-task plan on
# plain paths runs unasked, its finding's step is walked again and a pass
# closes the finding as fixed. A plan of four tasks, or one claiming a
# billing path, asks the owner once and starts only on `run`; `hold` and a
# lead that stores no plan leave their findings held. An idea starts nothing.
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"
unset WORKFLOW_STATUS_FILE
mkdir -p "$XDG_CONFIG_HOME/qshell"
printf 'here\n' >"$XDG_CONFIG_HOME/qshell/machine"

# The fake session, keyed on its brief. A walk passes. A findings lead
# stores the plan its finding's text names under the slug in its brief:
# `small` one task, `four` and `hold` four tasks, `billing` a task on
# app/billing/tax.php, `none` nothing. A task commits the file its Files line claims.
write_exec "$T_TMP/fake-worker.sh" <<'FAKE'
#!/bin/sh
task=$1; status=$3; brief=$4
done_json() { printf '{"is_error":false,"result":"ok"}\n'; }
goal=$(sed -n '/^## GOAL/,/^## SCOPE/p' "$brief")
case "$goal" in
*'Walk the Show path'*)
	printf 'walk\n' >>"$WF_TMP/sessions.log"
	cp "$brief" "$WF_TMP/walk.md"
	workflow report ready pass
	done_json
	exit 0
	;;
*'Turn the open findings into fix tasks'*)
	slug=$(grep -o 'mem plan fix-[a-z0-9]*' "$brief" | head -1 | cut -d' ' -f3)
	kind=$(sed -n '/^### Open findings/,/^##/p' "$brief" | grep -o 'fix: [a-z]*' | head -1 | cut -d' ' -f2)
	printf 'lead %s %s\n' "$slug" "$kind" >>"$WF_TMP/sessions.log"
	cp "$brief" "$WF_TMP/lead-$kind.md"
	task() { printf -- '- [ ] %s Fix part %s\n      Files: %s\n      Verify: true\n' "$1" "$1" "$2"; }
	{
		printf '# plan: %s\n\n' "$slug"
		case "$kind" in
		small) task "$slug-1" "app/$slug-1.php" ;;
		four | hold) for n in 1 2 3 4; do task "$slug-$n" "app/$slug-$n.php"; done ;;
		billing) task "$slug-1" app/billing/tax.php ;;
		esac
	} >"$WF_TMP/plan-$kind.md"
	[ "$kind" = none ] || mem plan "$slug" --stdin <"$WF_TMP/plan-$kind.md" >/dev/null
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
file=$(sed -n '/^## GOAL/,/^## SCOPE/p' "$brief" | grep -o 'Files: [^ ]*' | head -1 | cut -d' ' -f2)
mkdir -p "$(dirname -- "$file")"
printf '%s\n' "$task" >"$file"
git add "$file"
git -c core.hooksPath=/dev/null commit -qm "Fix a part"
say ready
done_json
FAKE
# The pid file is also written before the dispatch returns, so a tick that
# waits on it never looks before the session has started.
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$WF_TMP/fake-worker.sh" {task} {worktree} {status} {brief}'"'"' > {out} 2> {err} & echo $! > {pidfile}'
export WORKFLOW_DEADLINE_MIN=0.5

new_repo app
mem_register
"$MEM_BIN" project set verify true >/dev/null
printf '# roadmap: app-road\n\n- [x] m1 The home page\n      Surface: web\n      Show: open the home page, the heading reads Hello, the footer shows the year\n' |
	"$MEM_BIN" roadmap --stdin >/dev/null
"$MEM_BIN" roadmap --status maintenance >/dev/null

# finding <kind> -- an open finding on step 2 of m1, its short id printed.
finding() {
	sleep 0.05
	"$MEM_BIN" finding add --milestone m1 --step 2 "fix: $1 the heading reads Helo" | grep -o '^#[0-9A-Z]*' | tr -d '#'
}
small=$(finding small)
four=$(finding four)
billing=$(finding billing)
hold=$(finding hold)
none=$(finding none)
lower() { printf '%s' "$1" | tr '[:upper:]' '[:lower:]'; }

S="$XDG_STATE_HOME/workflow/serve/app"
mem_app() { "$MEM_BIN" --project app "$@"; }
status_of() { mem_app finding list --json | jq -r --arg id "$1" '.items[] | select(.short_id == $id) | .status'; }
question_on() { mem_app questions --for human --json | jq -r --arg id "$1" '[.questions[] | select(.body | contains("finding #" + $id))] | length'; }
question_id() { mem_app questions --for human --json | jq -r --arg id "$1" '.questions[] | select(.body | contains("finding #" + $id)) | .short_id' | head -1; }
sessions() { grep -c "^$1" "$T_TMP/sessions.log" 2>/dev/null; }

# tick -- one serve tick, then wait on every session and run it started.
tick() {
	(cd "$T_TMP" && workflow serve --once >>"$T_TMP/serve.out" 2>&1)
	local f pid
	for f in lead.pid dogfood.pid child.pid; do
		for _ in $(seq 300); do
			pid=$(cat "$S/$f" 2>/dev/null)
			[ -n "$pid" ] && kill -0 "$pid" 2>/dev/null || break
			sleep 0.2
		done
	done
}
# ticks_until <n> <command...> -- tick until the command holds, at most n times.
ticks_until() {
	local n=$1
	shift
	for _ in $(seq "$n"); do
		"$@" && return 0
		tick
	done
	"$@"
}
fixed() { [ "$(status_of "$1")" = fixed ]; }
asked() { [ "$(question_on "$1")" -ge 1 ]; }
held() { grep -qx "$1" "$S/held" 2>/dev/null; }

trap 'st=$?; (exit $st); t_done' EXIT

## ------------------------------------------------- the small fix, unasked

ticks_until 8 fixed "$small"
is "$(status_of "$small")" fixed 'the one-task fix closes its finding'
like "$(mem_app finding list --json | jq -r --arg id "$small" '.items[] | select(.short_id == $id) | .fixed_by')" \
	"$(git -C "$T_TMP/app" rev-parse HEAD)" 'as fixed by the trunk head'
is "$(question_on "$small")" 0 'with no question to the owner'
is "$(sessions "task fix-$(lower "$small")-1")" 1 'its one task ran'
is "$(sessions walk)" 1 'and its step was walked again'
like "$(sed -n '/^## GOAL/,/^## SCOPE/p' "$T_TMP/walk.md" 2>/dev/null)" '2\. the heading reads Hello' 'the walk names the finding'"'"'s step'
unlike "$(sed -n '/^## GOAL/,/^## SCOPE/p' "$T_TMP/walk.md" 2>/dev/null)" '1\. open the home page' 'and no other'
like "$(cat "$T_TMP/lead-small.md" 2>/dev/null)" "mem plan fix-$(lower "$small") --stdin" 'the lead is told to store one fix plan'
like "$(mem_app log --type run)" 'dogfood m1: pass' 'the walk is logged'
like "$(mem_app roadmap)" '^- \[x\] m1 ' 'the roadmap stays ticked'
is "$(mem_app roadmap --json | jq -r .status)" maintenance 'and in maintenance'

## ------------------------------------------------ four tasks: ask, then run

ticks_until 6 asked "$four"
tick
tick
is "$(question_on "$four")" 1 'a four-task plan asks the owner once'
like "$(mem_app questions --for human --json | jq -r '.questions[].body')" \
	"Run fix plan fix-$(lower "$four") for finding #$four\\? 4 tasks, more than three" 'and says why'
is "$(sessions "task fix-$(lower "$four")")" 0 'nothing of it runs before the answer'
like "$(cat "$S/fix" 2>/dev/null)" "^$four asked " 'the fix waits on the question'
unlike "$(mem_app plan 2>/dev/null | head -1)" "fix-$(lower "$four")" 'the asked plan is not the plan of record'
mem_app answer "$(question_id "$four")" run >/dev/null
ticks_until 8 fixed "$four"
is "$(sessions "task fix-$(lower "$four")")" 4 'on run its four tasks ran'
is "$(status_of "$four")" fixed 'and its finding is fixed after the walk'

## ------------------------------------------- a billing path: ask, then run

ticks_until 6 asked "$billing"
tick
like "$(mem_app questions --for human --json | jq -r '.questions[].body')" \
	"for finding #$billing\\? 1 tasks, app/billing/tax.php is a review path" 'a billing path asks and names it'
is "$(sessions "task fix-$(lower "$billing")")" 0 'nothing of it runs before the answer'
mem_app answer "$(question_id "$billing")" run >/dev/null
ticks_until 8 fixed "$billing"
is "$(question_on "$billing")" 1 'asked once'
is "$(sessions "task fix-$(lower "$billing")")" 1 'on run its task ran'
is "$(status_of "$billing")" fixed 'and its finding is fixed'

## ---------------------------------------------------------- hold, planless

ticks_until 6 asked "$hold"
mem_app answer "$(question_id "$hold")" hold >/dev/null
ticks_until 8 held "$none"
held "$hold" && ok 'hold leaves its finding held' || notok 'hold leaves its finding held' "$(cat "$S/held" 2>/dev/null)"
held "$none" && ok 'a lead that stores nothing leaves its finding held' || notok 'a lead that stores nothing leaves its finding held' "$(cat "$S/held" 2>/dev/null)"
is "$(sessions "task fix-$(lower "$hold")")" 0 'a held plan runs nothing'
is "$(status_of "$hold")" open 'and its finding stays open'
is "$(status_of "$none")" open 'so does the planless one'
is "$(sessions "lead fix-$(lower "$none")")" 1 'the planless finding had one lead'

## ------------------------------------------------------------------ an idea

"$MEM_BIN" idea 'a dark mode for the home page' >/dev/null
before=$(wc -l <"$T_TMP/sessions.log")
tick
tick
is "$(wc -l <"$T_TMP/sessions.log")" "$before" 'an idea starts nothing'
[ -e "$S/fix" ] && notok 'and no fix is going' "$(cat "$S/fix")" || ok 'and no fix is going'
is "$(cat "$S/stage" 2>/dev/null)" maintenance 'the stage is maintenance'
is "$(sessions walk)" 3 'three walks in all'
