---
name: fix
description: Use to diagnose a finding or a twice-failed task: a tight loop that shows red, three ranked hypotheses, a failing regression test, then the fix.
---

# fix

A fix starts as a diagnosis. Code changed before a loop shows red lands a
fix that may never touch the cause.

A fix for a finding opens from the finding and its evidence file:
`mem finding list --open --json` gives its step, its text and the `file`
the walk captured. That capture is the red the tight loop has to show.

## 1. Build the tight loop

One command that shows red on the defect, runs in seconds, and gives the same
answer every run: a targeted test, a script against the binary, a request
against the running service. Narrow it until it is fast; a slow loop gets run
too rarely to steer by.

Done when the command exits nonzero on the defect twice in a row and each run
takes under ten seconds.

## 2. Reproduce and minimise

Cut the input, the setup and the steps one at a time, rerunning the tight
loop after each cut.

Done when removing any remaining piece makes the red go away.

## 3. Rank three hypotheses

Write three candidate causes, each with the observation that would confirm or
kill it. Rank them by likelihood, then by how cheap the check is.

Done when three hypotheses are written and each one's check tells it apart
from the other two.

## 4. One variable at a time

Test the top hypothesis by changing one thing: a probe, an input, one line of
code. Revert it before testing the next. When all three die, go back to step 3
with what the checks showed.

Done when one hypothesis explains the red and changing that one thing alone
turns the tight loop green.

## 5. Regression test first

Write the regression test at the lowest seam that reaches the cause, and watch
it fail for the reason the winning hypothesis gives. Then write the fix.

Done when the test was red before the fix, is green after it, and `workflow
verify` exits 0.

## 6. Clean up and commit

Remove every probe: log lines, scratch scripts, loosened timeouts. Run
`workflow hygiene --staged` before the commit. The commit body states the
winning hypothesis in plain words: what was wrong, why the symptom followed,
and what the test pins.

Done when `git diff` holds only the fix and its test, and the commit body
names the cause.

## 7. Show it again

A fix for a finding ends on its `Show:` step captured again, the way the
walk captured it, and filed:

    mem evidence add --task <id> <file> --note "<what it shows now>"

The walk after the landing walks that step again, and on a pass
the engine closes the finding.

Done when the new capture is filed and shows the step the finding failed
working.
