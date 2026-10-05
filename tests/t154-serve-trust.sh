#!/usr/bin/env bash
# A walk waiting at a prompt is not read as ended. On claude's folder-trust
# screen serve answers once through amx, with the number of the option that
# says yes, and the walk stands. A trust screen with no yes to press is
# stopped, said once in the project's log, and the walk is skipped.
source "$(dirname -- "$0")/lib.sh"
t_init

mkdir -p "$XDG_CONFIG_HOME/qshell"
printf 'here\n' >"$XDG_CONFIG_HOME/qshell/machine"

export AMX_DIR="$T_TMP/amx"
mkdir -p "$AMX_DIR"
argv="$AMX_DIR/argv"
: >"$argv"

# A stand-in for amx. A walk's first session sits on the screen
# `$AMX_DIR/screen.<project>` draws, and stays on it whatever is pressed;
# a later walk works on and never ends. A lead ends at once, and a task
# commits its file and reports ready.
write_exec "$T_TMP/fake-amx" <<'AMX'
#!/bin/sh
(IFS='|'; printf '%s\n' "$*") >>"$AMX_DIR/argv"

verb=$1
shift
case $verb in
new)
	case " $* " in *" --check "*) exit 0 ;; esac
	name= dir= role= text=
	while [ $# -gt 0 ]; do
		case $1 in
		--name) name=$2; shift 2 ;;
		--dir) dir=$2; shift 2 ;;
		--role) role=$2; shift 2 ;;
		--model | --parent | --effort) shift 2 ;;
		--no-worktree | --bg | --json) shift ;;
		*) text=$1; shift ;;
		esac
	done
	case $role in
	dogfood)
		if mv "$AMX_DIR/screen.$MEM_PROJECT" "$AMX_DIR/$name.screen" 2>/dev/null; then
			printf 'waiting\n' >"$AMX_DIR/$name.state"
		else
			printf 'working\n' >"$AMX_DIR/$name.state"
		fi
		exit 0
		;;
	lead)
		printf 'done\n' >"$AMX_DIR/$name.state"
		exit 0
		;;
	esac
	brief=${text#Read }
	brief=${brief% and execute it exactly.}
	status=$(grep -oE '[^ ]+\.status' "$brief" | head -1)
	task=$(basename "$status" .status)
	cd "$dir" || exit 1
	mkdir -p app
	printf '%s\n' "$task" >"app/$task.php"
	git add "app/$task.php"
	git -c core.hooksPath=/dev/null commit -qm "Add a service"
	printf '%s ready\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$status"
	printf 'done\n' >"$AMX_DIR/$name.state"
	;;
status)
	[ -f "$AMX_DIR/$1.state" ] || exit 1
	state=$(cat "$AMX_DIR/$1.state")
	screen=
	if [ "$state" = waiting ] && [ -f "$AMX_DIR/$1.screen" ]; then
		screen=",$(cat "$AMX_DIR/$1.screen")"
	fi
	printf '{"id":"%s","state":"%s","last_event":0,"session":"","question":null%s}\n' \
		"$1" "$state" "$screen"
	;;
stop)
	printf 'stopped\n' >"$AMX_DIR/$1.state"
	;;
esac
exit 0
AMX
export WORKFLOW_AMX="$T_TMP/fake-amx"

# project <name> <screen>: a checkout whose one milestone has a Show line
# and a plan of one task, and whose first walk sits on <screen>.
project() {
	new_repo "$1"
	mem_register
	"$MEM_BIN" project set verify true >/dev/null
	printf '# roadmap: %s-road\n\n- [ ] m1 The home page\n      Show: open the home page, the heading reads Hello\n' "$1" |
		"$MEM_BIN" roadmap --stdin >/dev/null
	"$MEM_BIN" roadmap --status approved >/dev/null
	"$MEM_BIN" plan m1 --stdin >/dev/null <<-EOF
	# plan: m1

	- [ ] a1 Add the a1 service
	      Files: app/a1.php
	      Verify: true
	EOF
	printf '%s\n' "$2" >"$AMX_DIR/screen.$1"
}

project app '"kind":"trust","options":["No, exit","Yes, I trust this folder"]'
project wary '"kind":"trust","options":["Trust this folder","Exit"]'

N="$XDG_STATE_HOME/workflow/serve"
runs() { "$MEM_BIN" --project "$1" log --type run 2>/dev/null; }
# session <project>: the walk's session, off its walk file.
session() { head -1 "$N/$1/walk" 2>/dev/null | cut -d' ' -f4; }

cd "$T_TMP" || exit 1
WORKFLOW_DEADLINE_MIN=0.5 workflow serve --tick 1 >"$T_TMP/serve.out" 2>&1 &
serve_pid=$!
trap 'st=$?; kill "$serve_pid" 2>/dev/null; wait "$serve_pid" 2>/dev/null; (exit $st); t_done' EXIT

## ------------------------------------------------- a yes option to press

for _ in $(seq 120); do
	[ -f "$AMX_DIR/$(session app).screen" ] && break
	sleep 1
done
id=$(session app)
for _ in $(seq 30); do grep -q "^answer|$id|" "$argv" && break; sleep 1; done
# Past two more ticks, for a second answer or a walk read as ended.
sleep 3
is "$(grep -c "^answer|$id|" "$argv")" 1 'the trust screen is answered once'
is "$(grep -c "^answer|$id|2$" "$argv")" 1 'with the number of the option that says yes'
like "$(cat "$N/app/trusted" 2>/dev/null)" "^$id " 'the session is noted as trusted'
is "$(grep -c "^stop|$id$" "$argv")" 0 'the session is not stopped'
is "$(cat "$N/app/stage" 2>/dev/null)" dogfood 'the stage holds dogfood'
is "$(runs app | grep -c 'dogfood m1:')" 0 'and the walk is not read as ended'

## ---------------------------------------------------- no yes option to press

for _ in $(seq 60); do
	runs wary | grep -q 'dogfood m1:' && break
	sleep 1
done
like "$(runs wary)" 'dogfood m1: skipped no report' 'a trust screen with no yes skips the walk'
wid=$(grep -oE '^new\|--name\|[^|]+' "$argv" | cut -d'|' -f3 | while read -r n; do
	[ -f "$AMX_DIR/$n.screen" ] && grep -q '"Exit"' "$AMX_DIR/$n.screen" && echo "$n"
done)
is "$(grep -c "^stop|$wid$" "$argv")" 1 'the session is stopped'
is "$(grep -c "^answer|$wid|" "$argv")" 0 'with nothing pressed'
sleep 2
# The run log cuts a long line's title short; serve says it whole on stderr.
is "$(runs wary | grep -c 'serve: the walk of m1 sat on the folder-trust screen of ')" 1 \
	'and said once in the log'
is "$(grep -cF "wary: serve: the walk of m1 sat on the folder-trust screen of $T_TMP/wary -- open claude there once and accept it" "$T_TMP/serve.out")" 1 \
	'naming the checkout and what to do there'
