#!/usr/bin/env bash
# The dispatch, end to end, with a fake standing in for amx: the credential
# scrub of spec §1 on the environment the pane gets, the argv `amx new` is
# handed, a fresh handle per dispatch, the brief's contents, and a project
# whose path has a space in it.
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"
export AMX_DIR="$T_TMP/amx"
mkdir -p "$AMX_DIR"

# The fake does on `new` what the worker it starts would: reads the brief it
# was pointed at, writes the one file the task owns, commits it and reports
# ready. The environment and argv of every `new` are kept for the checks.
write_exec "$T_TMP/fake-amx" <<'AMX'
#!/bin/sh
verb=$1
shift
case $verb in
new | sub)
	env >"$AMX_DIR/env"
	(IFS='|'; printf '%s\n' "$*") >>"$AMX_DIR/argv"
	name= dir= text=
	while [ $# -gt 0 ]; do
		case $1 in
		--name) name=$2; shift 2 ;;
		--dir) dir=$2; shift 2 ;;
		--model | --effort) shift 2 ;;
		--no-worktree | --bg | --json) shift ;;
		--parent | --role) shift 2 ;;
		*) text=$1; shift ;;
		esac
	done
	[ "$verb" = sub ] && printf '{"id":"%s","parent":null,"phase":"starting","answer":null,"evidence":"hooks"}\n' "$name"
	brief=${text#Read }
	brief=${brief% and execute it exactly.}
	[ -r "$brief" ] || { printf 'no brief\n' >&2; exit 1; }
	task=$(basename "$brief" .md)
	status=$(sed -n 's/^Append one line per state change to \(.*\):$/\1/p' "$brief")
	file=$(sed -n 's/^ *Files: *//p' "$brief" | head -1 | cut -d' ' -f1)
	cd "$dir" || exit 1
	printf '%s started\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$status"
	mkdir -p "$(dirname "$file")"
	printf '%s\n' "$task" >"$file"
	git add "$file"
	git -c core.hooksPath=/dev/null commit -qm "Add the $task file"
	printf '%s ready merge-ready\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$status"
	printf 'done\n' >"$AMX_DIR/$name.state"
	;;
status)
	[ -f "$AMX_DIR/$1.state" ] || exit 1
	printf '{"id":"%s","state":"%s","evidence":"record","last_event":0,"session":""}\n' "$1" "$(cat "$AMX_DIR/$1.state")"
	;;
stop) printf 'stopped\n' >"$AMX_DIR/$1.state" ;;
esac
exit 0
AMX
export WORKFLOW_AMX="$T_TMP/fake-amx"

new_repo app
mem_register
printf '{"name":"acme/app"}\n' >composer.json
printf '#!/bin/sh\nexit 0\n' >artisan
chmod +x artisan
write_exec bin/php <<-'PHP'
	#!/bin/sh
	exit 0
PHP
git add -A
git -c core.hooksPath=/dev/null commit -qm 'project files'

cat >"$T_TMP/plan.md" <<'PLAN'
# plan: dispatch-check

## Rulings

- Ruling 1. Prices are integers in cents, never a float.

- [ ] t1 Extract cart pricing into a service
      Files: app/Services/Cart.php tests/Unit/Cart*
      Verify: bin/php artisan test --filter=Cart
      Done: cart totals identical for the fixture basket
- [ ] t2 A second task so the run is worth having
      Files: app/Services/Other.php
      Verify: true
PLAN

# The interactive environment carries live tokens. These stand in for them --
# including the ones with a digit in the name, which the glob-shaped contract of
# spec §1 covers and a letters-only character class quietly did not.
export GITHUB_API_KEY=live-token-1 GH_TOKEN=live-token-2 AWS_ACCESS_KEY_ID=live-token-3
export STRIPE_SECRET=live-token-4 SOME_APP_TOKEN=live-token-5 A_PRIVATE_KEY=live-token-6
export AWS_S3_SECRET=live-token-7 OAUTH2_TOKEN=live-token-8 GH2_TOKEN=live-token-9
export S3_SECRET=live-token-10
export ORDINARY_SETTING=keep-me

run env WORKFLOW_MAX_WORKERS=1 WORKFLOW_DEADLINE_MIN=0.5 WORKFLOW_MODEL=haiku \
	WORKFLOW_ALLOW_PUSH=1 WORKFLOW_HOOK_SEEN=/some/repo/.git \
	workflow run --plan-file "$T_TMP/plan.md"
is "$RC" 0 'the fake does the work, so both tasks merge'

## ------------------------------------------------------- the scrubbed env

envdump=$(cat "$AMX_DIR/env")
unlike "$envdump" '^GITHUB_API_KEY=' 'GITHUB_API_KEY never reaches the pane'
unlike "$envdump" '^GH_TOKEN=' 'nor does a GH_ variable'
unlike "$envdump" '^AWS_ACCESS_KEY_ID=' 'nor an AWS_ one'
unlike "$envdump" '^STRIPE_SECRET=' 'nor a STRIPE_ one'
unlike "$envdump" '^SOME_APP_TOKEN=' 'nor anything ending in _TOKEN'
unlike "$envdump" '^A_PRIVATE_KEY=' 'nor anything ending in _KEY'
unlike "$envdump" '^AWS_S3_SECRET=' 'nor an AWS_ name with a digit in it'
unlike "$envdump" '^OAUTH2_TOKEN=' 'nor a _TOKEN name with a digit in it'
unlike "$envdump" '^GH2_TOKEN=' 'nor a GH-prefixed name with a digit in it'
unlike "$envdump" '^S3_SECRET=' 'nor a _SECRET name that starts with a digit-bearing word'
unlike "$envdump" 'live-token' 'no token value survives anywhere in the environment'
like "$envdump" '^ORDINARY_SETTING=keep-me' 'and an ordinary variable is left alone'
like "$envdump" '^WORKFLOW_AGENT=1' 'WORKFLOW_AGENT reaches the pane'
like "$envdump" '^WORKFLOW_TASK=dispatch-check/t2' 'and so does the task tag'
unlike "$envdump" '^WORKFLOW_ALLOW_PUSH=' \
	'an orchestrator run under the push valve does not hand it to its workers'
unlike "$envdump" '^WORKFLOW_HOOK_SEEN=' \
	'nor the depth guard, which would tell the first commit the gate had run'

## ------------------------------------------------------------- the argv

rundir="$XDG_STATE_HOME/workflow/runs/app/dispatch-check"
s1=$(cat "$rundir/t1.session")
s2=$(cat "$rundir/t2.session")
like "$s1" '^wf-t1-[0-9a-z]{4}$' 'the handle is the agent name amx was told to pin'
isnt "$s1" "$s2" 'each dispatch gets its own'
wt="$XDG_STATE_HOME/workflow/worktrees/app/dispatch-check"
like "$(cat "$AMX_DIR/argv")" "^--name\\|$s1\\|--dir\\|$wt/t1\\|--no-worktree\\|--role\\|worker\\|--model\\|haiku\\|Read $XDG_CACHE_HOME/workflow/briefs/app/dispatch-check/t1.md and execute it exactly.$" \
	'amx new: the name, the task worktree, no second worktree, the model, and the brief as the task'
unlike "$(cat "$AMX_DIR/argv")" '--effort' 'no effort dial when none is set'
truthy "$([ ! -e "$rundir/costs.tsv" ] && echo 0 || echo 1)" \
	'no cost ledger: the run keeps no dollar figures'

## ------------------------------------------------------------- the brief

brief="$XDG_CACHE_HOME/workflow/briefs/app/dispatch-check/t1.md"
truthy "$([ -f "$brief" ] && echo 0 || echo 1)" 'the brief was written where the argv points'
unlike "$OUT" 'over the [0-9]+ byte budget' 'the block is inside its byte budget'
body=$(cat "$brief")
like "$body" 'Extract cart pricing into a service' 'the brief carries the objective'
like "$body" 'Files: app/Services/Cart\.php tests/Unit/Cart\*' 'and the task block verbatim'
like "$body" 'Verify: bin/php artisan test --filter=Cart' 'including the evidence command'
like "$body" 'Done: cart totals identical' 'and the done condition'
like "$body" 'The plan this task belongs to' 'the plan rides in the brief, after the block and the rules'
like "$body" 'Ruling 1\. Prices are integers in cents, never a float\.' 'with its rulings verbatim'
like "$body" 'Never leave it' 'the worktree boundary'
like "$body" 'never .git add -A.' 'the staging rule'
like "$body" 'mem ask' 'the stop protocol'
like "$body" 'started, progress, ready, blocked' 'and the reporting states'
like "$body" "$rundir/t1.status" 'the brief names the status file by path'
like "$body" '`workflow verify --gate` runs after merge' \
	'the brief names the gate that runs after merge'
like "$body" "the project's verify key, else" \
	"naming it the project's verify key, else"
like "$body" 'cargo test && cargo clippy -- -D warnings && cargo fmt --check' \
	'naming the Rust ladder as the example'
like "$body" 'A green Verify with a red gate fails the task' \
	'and that a green Verify does not excuse a red gate'
like "$body" 'run it before `ready`' \
	'and telling the worker to run the gate before it reports ready'

## ------------------------------------------- a project whose path has a space

# The same dispatch into a project called "my project": the worktree, the
# brief and the status path all carry that space, and amx takes each as one
# argument.
new_repo 'my project'
spaced=$PWD
mem_register
printf '{"name":"acme/spaced"}\n' >composer.json
printf '#!/bin/sh\nexit 0\n' >artisan
chmod +x artisan
write_exec bin/php <<-'PHP'
	#!/bin/sh
	exit 0
PHP
git add -A
git -c core.hooksPath=/dev/null commit -qm 'project files'
spaced_base=$(git rev-parse HEAD)

cat >"$T_TMP/spaced-plan.md" <<'PLAN'
# plan: spaced

- [ ] t1 Write the first file
      Files: app/One.php
      Verify: true
- [ ] t2 Write the second file
      Files: app/Two.php
      Verify: true
PLAN

is "$(printf '%s' "$spaced" | grep -c ' ')" 1 'the project really is at a path with a space in it'
run env WORKFLOW_MAX_WORKERS=2 WORKFLOW_DEADLINE_MIN=0.5 \
	workflow run --plan-file "$T_TMP/spaced-plan.md"
is "$RC" 0 'a project path with a space dispatches and the run completes'
srun="$XDG_STATE_HOME/workflow/runs/my project/spaced"
is "$(cat "$srun/t1.state")" merged 'spaced path: the first task merged'
is "$(cat "$srun/t2.state")" merged 'spaced path: and so did the second'
is "$(git rev-list --count "$spaced_base..integration/spaced")" 2 \
	'spaced path: both commits are on the integration branch'
is "$(wc -l <"$srun/t1.err")" 0 'spaced path: the worker wrote nothing to stderr'

t_done
