---
name: work
description: Use in a task worktree to do one plan task: failing test, smallest code, workflow verify, Show evidence, a commit in a human voice, then workflow report.
---

# work

One session does one task and ends. The brief carries the task; this skill
carries the method. `workflow verify` is your evidence and `workflow report
ready` is your last act.

## 1. Read

Send `workflow report started "<one line>"`. Read the brief whole, then only
the files and wiki sections it names, in the ranges you need. Every file you
write matches the task's `Files:` patterns; the pre-commit hook refuses the
rest and the merge gate fails the task.

A task that cannot be done as written is a stop, asked now rather than at
its Verify. A block that reads two ways is not a stop: build the reading its
wording and the surrounding code most directly support, build no other, and
name it in the report.

Done when you can name the seam the test goes in, every file you will write
matches `Files:`, and nothing open is one of the stops.

## 2. Test, then code

For a behaviour change, write the failing test first at the seam the task
names. Its expected value is a literal from the spec, never recomputed the way
the code computes it. Watch it go red for the reason the task gives, then
write the smallest code that turns it green.

`workflow verify` runs the task's `Verify:` line under the machine's suite
lock, and the gate trusts it. Iterate on one targeted test; the full suite
runs only through `workflow verify`. Exit 0 is green, 1 failed, 2 no
verifier, 3 a test was removed. A red Verify is answered in the code it
tests, never by weakening the test.

Red twice on the same error: ask before a third try.

    workflow advise "<question>" --file <path>

It prints the advisor's answer and your turn goes on.

Done when `workflow verify` exits 0 and the test you wrote was red before your
code.

## 3. Show

When the task has a `Show:` line, capture the evidence exactly as written
there and attach each file:

    mem evidence add --task <id> <file> --note "<what it shows>"

Done when every `Show:` item has an evidence entry whose note says what a
reader sees in it.

## 4. Commit

Stage the files this task touched, each by name. Then:

    workflow hygiene --staged

It refuses a staged agent file and any reference to a plan, task, ruling,
memory id, agent, model or session; the gate runs it again on every commit.
Fix what it names and run it again. Commit each atomic change in the voice
below.

Done when `workflow hygiene --staged` exited 0 before every commit and `git
status` shows nothing of yours left out.

## 5. Correct the wiki

A wiki section your change made false is yours to rewrite before you report:

    mem wiki <slug>#<section> > section.md
    mem wiki <slug>#<section> --stdin --note "<what changed and why>" <section.md

The note is the page's history: it says what changed, not that something did.

Done when every page the brief names matches the code as it now stands.

## 6. Report

`workflow verify --gate` runs the ladder the gate runs after merge; a green
Verify with a red gate fails the task, so run it first. Then:

    workflow report ready "<what landed, the reading you built, follow-ups>"

A bug or missing behaviour the task does not name goes in this note as a
follow-up, not into the change. A reply without a tool call ends the session,
so until `ready` every reply carries one.

Done when `ready` is the last line in the task's status file.

## The stops

Five things are never yours to decide:

1. Irreversible: the change fails the git-revert test.
2. Security-sensitive: auth, permissions, secrets handling, crypto.
3. Outside the worktree: push, publish, deploy, any external write.
4. The plan is broken beyond guessing, including a file the change must
   touch that `Files:` omits.
5. Credentials or secrets in play.

On any of them, ask the smallest question that unblocks you, then stop:

    mem ask "<question>"
    mem handoff --set "<where you are>"
    workflow report blocked "<the question>"

The answer opens your next brief. Ask once.

## Commit and comment voice

The reader of a commit or a comment has only this repo. Write for that reader:

- The subject is under 72 characters, imperative and about the change:
  "Refuse an unknown effort level at plan check". The body gives the why.
- A comment gives the reason the code is this way, in a line or two.
- Name the actor: "the gate reruns hygiene", not "hygiene is rerun".
- Plain words: use, help, is. A number or a mechanism where an adverb wanted
  to go.
- A comma or a full stop where a dash wanted to go; no em or en dashes.
  Straight quotes.
- State what the change does. Puffery ("robust", "seamless", "key") and a
  closing summary come out.
- The reason stands on its own. The message and the code say nothing about
  plans, tasks, decisions, agents, models, memory or sessions, and carry no
  trailer.
