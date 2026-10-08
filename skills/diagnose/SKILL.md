---
name: diagnose
description: Use when a bug, a failing or flaky test, or a dogfood finding needs its cause found before it is fixed.
---

# diagnose

A fix starts as a diagnosis. Code changed before a tight loop shows red lands
a fix that may never touch the cause. In an interactive session, a stop goes
to Saiful in the chat; an orchestrator parks it with `mem ask --for human`.

## 1. Build the tight loop

A tight loop is one command that goes red on the exact symptom (the error
text, the wrong value, the slow time), runs in seconds, and gives the same
verdict every run. Build it in the first of these ways that reaches the bug:

1. a failing test at the seam where the bug happens
2. a script against the binary, or a request against the running service
3. a browser script that drives the page and asserts on what it shows
4. a captured payload or event replayed through the code path
5. `git bisect run` between a good and a bad commit

Pin time, seeds and the network so the verdict holds. A finding starts from
its evidence: `mem finding list --open --json` gives its step, its text and
the file the walk captured, and the loop shows that symptom. Show a secret
as `<REDACTED>`, and keep credentials in the environment.

A flaky failure needs a rate, not one red. Run it 20 times alone and 20
times inside its suite:

    for i in $(seq 20); do <command> >/dev/null 2>&1 && echo pass || echo FAIL; done | sort | uniq -c

Red only inside the suite is test-order pollution: bisect the tests that run
before it. Raise the rate (more runs, parallel runs, load) until it fails at
least one run in two. A rerun that passes proves nothing.

When no loop can be built, stop and say what you tried, and ask for access
to where it fails, a captured artifact, or leave to add temporary probes.
Form no hypothesis without a loop.

Done when the command ran twice, red both times on the symptom the report
names (a flaky one: its failure rate measured), each run in seconds.

## 2. Minimise

Cut input, setup, callers and steps one at a time, and rerun the loop after
each cut.

Done when removing any remaining piece makes the red go away.

## 3. Rank three hypotheses

Write three candidate causes, each with its prediction: "if X is the cause,
changing Y turns the loop green". Rank them by likelihood, then by how cheap
the check is, and record them: `mem log "diagnosing <symptom>: 1) ... 2) ...
3) ..."`. In the chat, show them to Saiful before you test, since he may know
which one it is, and carry on with your ranking.

Done when each hypothesis has a check that tells it apart from the other two.

## 4. One variable at a time

Test the top hypothesis by changing one thing: a probe, an input, one line.
Tag every probe line `[DEBUG-<4 hex>]` so one search removes them all. Revert
it before the next test. When all three die, go back to step 3 with what the
checks showed. For slowness, measure a baseline first and bisect against it.

A fix that fails kills its hypothesis. After two failed fixes, the design
is the suspect: stop and take what both showed to Saiful.

Done when one hypothesis explains the red, and changing that one thing alone
turns the tight loop green.

## 5. Regression test, then the fix

Turn the minimised case into a test at the seam where the bug happens, one
that drives the call path that broke. Watch it fail for the reason the
winning hypothesis gives, then fix the code and watch it pass. Rerun the
tight loop on the original, unminimised case.

A seam too shallow to show the real pattern gives false confidence. When no
seam reaches it, that is the finding: `mem idea "no test seam reaches <bug>:
<why>"`, and the tight loop is the fix's proof.

Done when the test was red before the fix and green after, and the tight
loop is green on the original case.

## 6. Clean up and commit

Remove every probe (`rg '\[DEBUG-'` finds nothing), restore any loosened
timeout, and keep scratch scripts in a `mktemp -d` directory. Commit in the
tdd skill's voice. The body states the cause: what was wrong, why the
symptom followed, and what the test pins. Close a finding with `mem finding
close <id> --by <commit>`.

Done when the diff holds only the fix and its test, the commit body names
the cause, and the finding is closed.
