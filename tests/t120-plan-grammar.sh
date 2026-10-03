#!/usr/bin/env bash
# plan-check holds a plan to the three keys that tell the engine how to run
# and show it: an Effort outside the levels amx accepts is refused before a
# worktree is cut for it, a milestone with no Surface (or one outside the
# list) is refused because the dogfood playbook is picked from it, and a task
# whose Files reach a surface with no Show line is warned. A plan carrying all
# three passes clean.
source "$(dirname -- "$0")/lib.sh"
t_init

new_repo grammar
mem_register

plan() { cat >"plan.md"; }
check() {
	OUT=$(workflow plan-check plan.md 2>"$T_TMP/check.err")
	RC=$?
	ERR=$(cat "$T_TMP/check.err")
}
mkdir -p road
road() { cat >"road/roadmap.md"; }
milestone() { cat >"road/$1.md"; }
check_road() {
	OUT=$(workflow plan-check road/roadmap.md 2>"$T_TMP/road.err")
	RC=$?
	ERR=$(cat "$T_TMP/road.err")
}

## -------------------------------------------------------- Effort refuses

plan <<'EOF'
# plan: grammar

Price the cart.

- [ ] t1 Price the cart
      Files: lib/cart.rs
      Effort: extreme
      Verify: true
EOF
check
is "$RC" 1 'an Effort outside the levels is refused'
like "$ERR" "plan: task t1: Effort 'extreme' -- one of low, medium, high, xhigh, max" \
	'the refusal quotes the value and lists the levels'

plan <<'EOF'
# plan: grammar

Price the cart.

- [ ] t1 Price the cart
      Files: lib/cart.rs
      Effort: xhigh
      Verify: true
EOF
check
is "$RC" 0 'a level amx accepts passes'
unlike "$ERR" 'Effort' 'and earns no finding'

## ---------------------------------------------- a surface asks for Show

plan <<'EOF'
# plan: grammar

Price the cart.

- [ ] t1 Show the cart
      Files: lib/cart.rs web/src/components/**
      Verify: true
- [ ] t2 Style the cart
      Files: site/cart.css
      Verify: true
- [ ] t3 Price the cart
      Files: lib/price.rs
      Verify: true
EOF
check
is "$RC" 0 'a surface task with no Show line is not refused'
like "$ERR" "plan: task t1: Files reach a surface \(web/src/components/\*\*\) and the task has no Show line -- say what evidence to capture" \
	'a surface directory in Files warns, naming the pattern'
like "$ERR" "plan: task t2: Files reach a surface \(site/cart.css\)" \
	'and so does a surface extension'
unlike "$ERR" 'task t3: Files reach a surface' 'a library path is no surface'

plan <<'EOF'
# plan: grammar

Price the cart.

- [ ] t1 Show the cart
      Files: web/src/components/**
      Show: open /cart and screenshot the total
      Verify: true
EOF
check
unlike "$ERR" 'Files reach a surface' 'a surface task with a Show line is not warned'

## ----------------------------------------------------- Surface refuses

road <<'EOF'
# roadmap: shop

- [ ] m1 The cart
EOF
milestone m1 <<'EOF'
# plan: m1

Price the cart.

- [ ] t1 Price the cart
      Files: lib/cart.rs
      Verify: true
- [ ] t2 Total the cart  [after: t1]
      Files: lib/total.rs
      Verify: true
EOF
check_road
is "$RC" 1 'a milestone with no Surface line is refused'
like "$ERR" 'roadmap: milestone m1: no Surface line -- web, mobile, cli, emacs or lib, so the engine knows how to drive it' \
	'the refusal lists the surfaces and why'
like "$ERR" 'roadmap: milestone m1: no Show line' 'and the Show warning stays'

road <<'EOF'
# roadmap: shop

- [ ] m1 The cart
      Surface: desktop
EOF
check_road
is "$RC" 1 'a Surface outside the list is refused'
like "$ERR" "roadmap: milestone m1: Surface 'desktop' -- web, mobile, cli, emacs or lib, so the engine knows how to drive it" \
	'the refusal quotes the value'

## --------------------------------------------------------- a clean plan

road <<'EOF'
# roadmap: shop

- [ ] m1 The cart
      Surface: web
      Show: Saiful opens /cart and sees the total
EOF
milestone m1 <<'EOF'
# plan: m1

Price the cart.

- [ ] t1 Price the cart
      Files: lib/cart.rs
      Effort: xhigh
      Verify: true
- [ ] t2 Show the total  [after: t1]
      Files: web/src/pages/cart.tsx
      Show: open /cart and screenshot the total
      Verify: true
EOF
check_road
is "$RC" 0 'a roadmap carrying Effort, Surface and Show passes'
unlike "$ERR" 'Surface' 'with no Surface finding'
unlike "$ERR" 'Effort' 'no Effort finding'
unlike "$ERR" 'Show line' 'and no Show finding'
