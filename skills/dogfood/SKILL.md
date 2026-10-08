---
name: dogfood
description: Use as the dogfooder subagent, or when asked to dogfood a milestone: walk the real product along its Show path the way a person would, and file each defect in mem with evidence.
---

# dogfood

You are the milestone's first user. The orchestrator gives you numbered
steps, one per action a person takes, plus the launch command and the
project's `verify` wiki page when there is one. Every step ends in a tick
(seen working on the real surface) or a finding. A tick is only what you
saw.

Keep the walk short: launch within 2 minutes, aim for 5 to 8 minutes and 12
steps plus a 3-minute edge pass, and stop at 15 minutes with what you have.

## 1. Pick the surface

Drive the product through the entry points a person uses, on a surface of
its own. Never use your shortcuts.

- **Web app.** An isolated browser through `playwright-cli`, started from a
  scratch directory (`cd "$(mktemp -d)"`) so its files never land in the
  repo. Check at 390 px and 1280 px wide.
- **CLI or TUI.** A tmux session of its own: `tmux new -d -s walk-<n>`, then
  `send-keys` and `capture-pane -p`.
- **The desktop shell.** A nested niri, as the `desktop` skill describes,
  never the live session.

Drive Saiful's live desktop only when he asked for it, or when the hub says
he is away and the screen is unlocked:

    curl -s http://127.0.0.1:8787/api/presence   # "watching": false and "locked": false

The `desktop` skill holds the tools and their rules (text before pixels,
region screenshots, OCR before reading an image). Load it before the walk.

## 2. Launch

Launch the product the way `mem wiki verify#launch` (or the orchestrator's
brief) says; the recipe for its kind of surface (web, mobile, cli, emacs,
lib) is in `mem --project workflow wiki dogfood-playbooks`. Then run one
read-only check that it is worth driving: a page
that loads, a `--help` that prints, a health URL. Build or start what is at
`HEAD`, and check that the binary or bundle is newer than the last commit.

A product that does not start is one finding on step 0, with the output,
and the walk ends there:

    mem finding add --milestone <slug> --step 0 "cannot launch: <what it said>" --evidence launch.txt

## 3. Walk

Take each step as a person would, then judge what the step shows. Read text
first: the page's accessibility snapshot, the pane, the console and network
logs. `jev check "<claim>"` judges a page and `jev pane <session> "<claim>"`
judges a terminal. jev exit 2 is a defect, and exit 3 means look yourself.
jev cannot see colour or layout, so judge those from a region screenshot.

Prove a mutation (a save, a send, a delete) with a read-only second view,
such as a reload, the list it should appear in, or a query. A confirmation
message alone proves nothing. Retry a step once when the walker itself
failed (a stale ref, a page still compiling), and do not file that as a
defect.

Capture each step: `playwright-cli screenshot --filename step-<n>.png` (a
region, not the full screen), or `tmux capture-pane -p > step-<n>.txt`.

Then the edge pass, about 3 minutes. At each step that takes input, enter a
300-character string with no spaces, an empty value, zero, and non-ASCII
text (`ünïcødé 漢字 🙂`), and judge the screen at 390 px as well. For each
write the brief names as a trust boundary (a form post, a webhook, a message
one party sends another), forge it the way an outsider would: the request
without the page, with no session, or replayed. Nothing may change. File
what breaks against the step it belongs to.

## 4. File each defect

One finding per defect, filed before cleanup (mem copies the file in):

    mem finding add --milestone <slug> --step <n> "<what happened>; expected <what the step says>; a test should assert <the check>" --evidence step-<n>.png

The "a test should assert" part lets the orchestrator write the red test.
A step you could not drive is a finding too, starting `undriveable: `
and saying what stopped you. Never file taste as a defect. Name it in your
report instead.

## 5. Map what worked

Each feature you drove gets a section on the `verify` wiki page. A section
says how to launch the product, how to reach the feature, how you drove it,
and the gotchas you hit. Keeping it current lets the next walk start in
seconds:

    mem wiki verify > "$d/verify.md"     # $d from mktemp -d; add or edit sections
    mem wiki verify --stdin --note "<feature>: <what changed>" < "$d/verify.md"

## 6. Clean up and report

Stop what you started (the server, the tmux session, the browser), by the
pid or name you kept. Then report to the orchestrator:

- `pass`, or `failed <n> <n>` with each failed step's finding id
- the walk's minutes and the steps taken
- anything that looked wrong but is taste, for Saiful to judge
