---
name: dogfood
description: Use when the engine starts you to use a milestone's real product along its Show path, judge each step on screen, and file every defect as a finding with evidence.
---

# dogfood

You are the milestone's first user. The Show path is the milestone's `Show:`
line, cut into numbered steps, one per action a user takes. Every step ends
in a tick (seen working on the real surface) or a finding. A step you cannot
drive is a finding of class `undriveable`; a tick is only what you saw.

## 1. Launch

The brief numbers the steps and carries the `verify` page's Launch, Doctor,
Drive, Evidence and Cleanup sections and the playbook for the Surface. A
section the playbook links to lives in the workflow project:

    mem --project workflow wiki dogfood-playbooks#<name>

Launch the product as Launch says, then run Doctor. A project with no
verify page is launched as its README says; keep what worked for step 4.

A Doctor that fails is one finding on step 0 with its output, and the walk
ends there; go on to step 5:

    mem finding add --milestone <slug> --step 0 "cannot launch: <what Doctor said>" --evidence doctor.txt

Done when Doctor passes, or the cannot-launch finding is filed.

## 2. Walk the Show path

A later walk takes only the steps the brief names, under their numbers on
the whole Show path; the rest passed before.

Take each step as a user would, on the real surface, with the tool Drive
names: the browser through `playwright-cli`, a terminal through a tmux pane,
never a shortcut the user does not have. Then judge what the step shows and
capture it:

    jev check "<what the step should show>"
    playwright-cli screenshot --filename step-<n>.png

On a terminal surface, `jev pane <session> "<claim>"` judges the pane and
`tmux capture-pane -p -t <session> > step-<n>.txt` is the capture. jev exit
2 is a defect; exit 3 means look at the capture yourself. jev reads text
only, so colour and layout are judged from the screenshot.

A mutation (a save, a send, a delete) is proven by a read-only second view:
a reload, the list it should appear in, a query. The confirmation message
alone proves nothing.

Done when every step has a jev answer, a capture, and a tick or a defect
noted against its number.

## 3. File each defect

One finding per defect, with the capture that shows it:

    mem finding add --milestone <slug> --step <n> "<what happened>; expected <what the Show path says>" --evidence step-<n>.png

A step you could not drive is filed the same way, its text starting
`undriveable: ` and saying what stopped you.

Done when every step without a tick has a finding with evidence.

## 4. Map what you drove

Each feature you drove gets a section on `verify`: how to get to it, how you
drove it, the gotchas you hit. An existing section is rewritten in place:

    mem wiki verify#<feature> > section.md
    mem wiki verify#<feature> --stdin --note "<feature>: <what changed>" < section.md

A section write refuses a heading the page lacks, so a new feature goes in
through the whole page: `mem wiki verify > verify.md`, add the `## <feature>`
section, `mem wiki verify --stdin --note "<feature>: first driven" < verify.md`.

A project with no verify page gets one here, written from what worked:
Launch, Doctor, Drive, Evidence and Cleanup, then a section per feature.
A wiki write refused off the runner machine is skipped; the findings and
the report carry the walk.

Done when every feature on the Show path has a current section on `verify`,
or the write was refused off the runner machine.

## 5. Clean up

Run Cleanup as the page says. mem copied each evidence file in when the
finding was filed; confirm that copy survived:
`mem finding list --open --json` shows a `file` on each of your findings.

Done when Cleanup has run and every finding you filed lists its file.

## 6. Report

The last act is one of these, the failed step numbers after `failed`:

    workflow report ready "pass"
    workflow report ready "failed <n> <n>"

A failed step is one with a finding filed on it; a cannot-launch walk
reports `failed 0`. The engine reads this line, logs the walk and closes the
findings a later walk sees fixed.

Done when the report is sent and every step it names failed has an open
finding.
