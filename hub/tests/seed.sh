# shellcheck shell=bash
# The seeded store the hub sandbox serves and the memview conformance test
# reads, sourced by both. The caller sets `sb`, the directory the checkouts
# go in, puts mem on PATH and points HOME and XDG_* at a throwaway store.
#
# Four projects cover the hub's stages: alpha has a brief only, beta a draft
# roadmap waiting on approval, gamma an approved milestone being built, with
# evidence, findings, a wiki and rulings, and delta a roadmap with every
# milestone ticked.

# project <name>: a checkout with one commit, cd'd into. mem registers it on
# its first write.
project() {
	mkdir -p "$sb/$1"
	cd "$sb/$1"
	git init -q .
	printf '# %s\n' "$1" >README.md
	git add README.md
	git -c core.hooksPath=/dev/null commit -qm 'Start the project'
}
# page <slug> <title>: a wiki page of three sections.
page() {
	printf '# %s\n\n## What\n\nWhat %s says, in a line.\n\n## Why\n\nThe reason it is so.\n\n## Open\n\nNothing open.\n' "$2" "$2" |
		mem wiki "$1" --stdin --note "write the $1 page" >/dev/null
}

project alpha
mem brief --set 'A shared shopping list two phones keep in step.' >/dev/null

project beta
mem brief --set 'A recipe box that scales a dish to the people eating it.' >/dev/null
mem roadmap --stdin >/dev/null <<'ROAD'
# roadmap: beta

- [ ] b1-box Recipes are stored and listed
      Show: a recipe added on the phone is listed on the laptop
- [ ] b2-scale A recipe scales to any number of people  [after: b1-box]
      Show: a recipe for four, set to six, lists half again of every amount
ROAD
mem roadmap --status draft >/dev/null
# b1-box's plan links the artifact it was drawn from, which its page opens.
mem plan b1-box --stdin >/dev/null <<'PLAN'
# plan: b1-box

Goal: a recipe saved on the phone shows in the list on the laptop.
Artifacts: [recipe box mockup](https://claude.ai/artifact/0f3c2b9e)

## Behaviours

### 1. A saved recipe shows in the list with its title

Acceptance: save "Dal" and the list's first row reads "Dal".
PLAN
mem plan b2-scale --stdin >/dev/null <<'PLAN'
# plan: b2-scale

- [ ] b2-t1 The first change
PLAN
# Asked the way the plan skill asks it.
mem ask --for human --options approve,changes --recommend approve 'Review the beta roadmap' >/dev/null

project gamma
mem brief --set 'A habit tracker that nags once and then lets go.' >/dev/null
for slug in index spec research-summary; do page "$slug" "gamma $slug"; done
# A word no other item carries, so a search for it has one hit, a section.
{ mem wiki spec; printf '\n## Quiet hours\n\nNo nag goes out between ten at night and seven in the morning.\n'; } |
	mem wiki spec --stdin --note 'add the quiet hours' >/dev/null
mem roadmap --stdin >/dev/null <<'ROAD'
# roadmap: gamma

- [x] g1-log Habits are logged from the phone
      Show: a habit ticked on the phone shows on the week view
- [ ] g2-nag A missed habit nags once
      Show: a habit missed by nine is nagged at nine and never again that day
ROAD
mem roadmap --status approved >/dev/null
mem plan --stdin >/dev/null <<'PLAN'
# plan: g2-nag

- [x] g2-t1 Store when a habit is due
- [ ] g2-t2 Send the nag
- [ ] g2-t3 Stop after one nag a day
PLAN
mem ask --for human --options 'nine,eight,ten' --recommend nine 'Which hour does the nag go out?' >/dev/null
# An answer of one word longer than a phone's line, so the questions page
# shows how an unbroken answer fits.
named=$(mem ask --for human 'What is the streak called?' | sed -n 's/^#\([^ ]*\).*/\1/p')
mem answer "$named" the_streak_kept_every_single_day_from_the_first_tick_to_the_last_without_a_break >/dev/null
# A week of seven days with one ticked, small enough to sit here as text,
# so the gallery has an image a screenshot can show without a browser.
base64 -d >"$sb/week.png" <<<'iVBORw0KGgoAAAANSUhEUgAAAKAAAAB4CAIAAAD6wG44AAABJElEQVR42u3RsQ0AIAgAQWdxJ5d0JisrS0dwCCwIuQ81CVy7Kl3zAsACLMACLMACLMCAVQl47ROcX0v6HMFJdU6GJYABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwEmBVTjAgAVYgAVYgAVYgAGrTA9RzR3lkHFmKgAAAABJRU5ErkJggg=='
mem evidence add --task g2-t1 "$sb/week.png" --note 'the week view with one habit ticked' >/dev/null
# A plain black screenshot as wide as a laptop's, 1280 by 720, so the
# gallery shows how it fits a phone.
base64 -d >"$sb/wide.png" <<<'iVBORw0KGgoAAAANSUhEUgAABQAAAALQAQAAAADnBuD7AAAAh0lEQVR42u3BMQEAAADCoPVPbQlPoAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAB4GsTfAAGc95RKAAAAAElFTkSuQmCC'
mem evidence add --task g2-t1 "$sb/wide.png" --note 'the week view on a laptop screen' >/dev/null
printf 'ticked at 00:00, listed under yesterday\n' >"$sb/midnight.txt"
printf 'the nag fires twice at nine\n' >"$sb/twice.txt"
mem finding add --milestone g1-log --step 1 --evidence "$sb/midnight.txt" 'a habit ticked at midnight lands on the wrong day' >/dev/null
mem finding add --milestone g2-nag --step 2 --evidence "$sb/twice.txt" 'the nag fires twice at nine' >/dev/null
fixed=$(mem finding list | sed -n 's/^#\([^ ]*\) .*wrong day.*/\1/p')
mem finding close --by "$(git rev-parse --short HEAD)" "$fixed" >/dev/null
mem decide --by saiful 'Nag by notification, not by email' >/dev/null
mem decide --by saiful --replaces 'Nag by notification, not by email' 'Nag by notification, once a day at most' >/dev/null

project delta
mem brief --set 'A command line that renames photos by the day they were taken.' >/dev/null
mem roadmap --stdin >/dev/null <<'ROAD'
# roadmap: delta

- [x] d1-rename Photos are renamed by date
      Show: a folder of camera names becomes a folder of dates
ROAD
mem roadmap --status approved >/dev/null
mem idea 'Read the date from a video file too' >/dev/null
mem handoff --set 'Nothing in flight; the last fix landed.' >/dev/null
