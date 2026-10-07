---
name: tdd
description: Use to build one task test-first, as an implementer subagent or an orchestrator working solo. Write the failing test, the smallest code that passes it, run the targeted tests and typecheck, and commit in a human voice.
---

# tdd

One task, one red-to-green loop, one or a few atomic commits. The brief, or
the orchestrator's checklist, carries the task. This skill carries the
method.

## 1. Read

Read the brief whole, then only the files and wiki sections it names, in the
ranges you need. For a library API you are not sure of, read its current
docs with `npx ctx7@latest library <name> "<query>"` and then `npx
ctx7@latest docs <id> "<query>"`. Do not guess, and do not read a package's
built `dist`.

A brief that reads two ways is not a stop. Build the reading the wording and
the surrounding code most directly support, build no other, and name it in
your report.

Done when you can name the seam the test goes in and every file you will
write.

## 2. Red

Write the failing test first, at the seam the task names. Call the code the
way its users do, and assert against a literal expected value from the spec
or the plan, never one recomputed the way the code computes it. Run it and
watch it fail for the reason the task gives. A test that passes before the
change tests nothing.

## 3. Green

Write the smallest code that turns the test green. No speculative options,
layers or handling for cases that cannot happen. Then run, and only run:

- the test file you touched, and any test file for code you changed
- the typecheck or the compiler

Seconds, not minutes. The full suite is the orchestrator's to run once per
wave. A red test is answered in the code it tests, never by weakening,
skipping or deleting the test.

The same error twice: stop guessing. Write down three ranked hypotheses,
test the cheapest one, and ask the advisor tool when Claude Code offers it.

Done when your test was red before your code and everything you ran is green.

## 4. Commit

Stage the files this task touched, each by name, never `git add -A`. The
pre-commit hook runs `workflow hygiene --staged`. It refuses agent files and
any reference to a plan, task, ruling, mem id, agent, model or session. Fix
what it names. Commit each atomic change in the voice below. Leave nothing
of yours unstaged.

## 5. Report

Your last message, to whoever sent you:

- the commits, as `sha subject`
- the test command, with its red output before and green output after, in
  short
- every decision you made that the brief did not, and why
- anything out of scope you noticed, as a follow-up, not a change

## The stops

Come back instead of improvising when the change would:

1. fail the git-revert test (irreversible),
2. touch auth, permissions, secrets or crypto,
3. reach outside the repo (push, publish, deploy, an external write),
4. need a file outside your SCOPE, or a plan fact that turns out false.

As a subagent, report the stop and end. Working solo, ask with `mem ask
--for human` and keep working on what does not depend on the answer.

## Commit and comment voice

The reader of a commit or a comment has only this repo. Write for that
reader.

- The subject is under 72 characters, imperative, and about the change:
  "Refuse an unknown effort level". The body gives the why.
- A comment gives the reason the code is this way, in a line or two.
- Name the actor: "the hook reruns hygiene", not "hygiene is rerun".
- Use plain words (use, help, is), and put a number or a mechanism where an
  adverb wanted to go.
- Use a comma or a full stop where a dash wanted to go. No em or en dashes.
  Use straight quotes.
- State what the change does. Leave out puffery ("robust", "seamless", "key")
  and any closing summary.
- The reason stands on its own. Nothing mentions plans, tasks, decisions,
  agents, models, memory or sessions, and there are no trailers.
