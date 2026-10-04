#!/usr/bin/env bash
# workflow dogfood asks the engine to walk a landed milestone again: one
# question for the orchestrator naming the milestone, the checkout's head and
# the machine to walk on. The milestone is the roadmap's last ticked one
# unless named, and the machine is the project's dogfood-machine key unless
# that is unset. No project, or no ticked milestone, is refused with 2.
source "$(dirname -- "$0")/lib.sh"
t_init

mkdir -p "$XDG_CONFIG_HOME/qshell"
printf 'here\n' >"$XDG_CONFIG_HOME/qshell/machine"

new_repo app
mem_register
APP="$T_TMP/app"
head=$(git rev-parse HEAD)

# The body of the pending question whose id dogfood printed, and who it is for.
asked() {
	"$MEM_BIN" --project app questions --pending --for orchestrator --json |
		jq -r --arg id "${OUT#\#}" '.questions[] | select(.short_id == $id) | "\(.audience): \(.body)"'
}
count() { "$MEM_BIN" --project app questions --pending --for orchestrator --json | jq '.questions | length'; }

## --------------------------------------------------------- no project

cd "$T_TMP" || exit 1
run workflow dogfood
is "$RC" 2 'no project named and none here exits 2'
like "$OUT" 'name a project' 'and says to name one'

## ---------------------------------------------- no ticked milestone

cd "$APP" || exit 1
printf '# roadmap: app-road\n\n- [ ] m1 The shop\n- [ ] m2 The cart\n' | "$MEM_BIN" roadmap --stdin >/dev/null
run workflow dogfood
is "$RC" 2 'a roadmap with nothing ticked exits 2'
like "$OUT" 'no ticked milestone' 'and says why'
is "$(count)" 0 'and asks nothing'

## ------------------------------------------- the last ticked milestone

printf '# roadmap: app-road\n\n- [x] m1 The shop\n- [x] m2 The cart\n- [ ] m3 The checkout\n' |
	"$MEM_BIN" roadmap --stdin >/dev/null
run_out workflow dogfood
is "$RC" 0 'dogfood exits 0 once it has asked'
like "$OUT" '^#[0-9A-Z]{8}$' 'and prints the question id'
is "$(asked)" "orchestrator: dogfood m2 at $head on here" 'the engine is asked for the last ticked milestone at the head, on this machine'

## ------------------------------------------- a milestone named, from outside

cd "$T_TMP" || exit 1
run_out workflow dogfood app --milestone m1
is "$RC" 0 'a project and a milestone named from outside the checkout'
is "$(asked)" "orchestrator: dogfood m1 at $head on here" 'the engine is asked for the milestone named at the checkout head'

## ------------------------------------------------ the dogfood machine

cd "$APP" || exit 1
"$MEM_BIN" project set dogfood-machine mini >/dev/null
run_out workflow dogfood
is "$RC" 0 'dogfood with the key set exits 0'
is "$(asked)" "orchestrator: dogfood m2 at $head on mini" 'and asks for the walk on the machine the key names'
is "$(count)" 3 'one question per request'
