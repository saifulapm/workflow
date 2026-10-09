---
name: grill
description: "Use when Saiful brings a new product, feature or big change: interview him in rounds until no decision is open, then write the spec the plan skill cuts milestones from."
---

# grill

The topic is a tree of decisions. The frontier is every open decision whose
prerequisites are settled. Each round asks the whole frontier. Decisions are
Saiful's. Facts are yours to find. The grilling ends when the frontier is
empty and he confirms the summary. Then the spec is written from the record
with no second approval.

Everything goes in mem. Nothing goes in the repo.

## 1. Research, when the product is new

For a new product or a feature with prior art, send up to three subagents in
parallel before the first round. One covers competitors and how they solve it,
one covers the libraries and APIs it will stand on (current docs through `npx
ctx7@latest`, dated), and one covers the existing code it touches. Keep what matters on wiki
pages (`research-<topic>`) and a `research-summary` that ends with the
questions only Saiful can answer. A small change in a known codebase skips
this step.

Done when the research summary ends with those questions, or the change is
small enough to skip.

## 2. Read

Read `mem brief`, the research summary, the glossary (`mem wiki glossary`)
and the decisions already recorded (`mem search "<topic>"`). Build the tree.
Each node is a decision, and each edge is a decision it depends on.

Sort every decision into one of three kinds:

- **mechanical**: one right answer exists. Take it, and record it with
  `--by agent`.
- **taste**: a reasonable default exists. Propose it in the round.
- **owner**: only Saiful knows, such as money, users, scope or risk. Ask.

Done when every decision has its kind and names the decisions it waits on.

## 3. Ask a round

Ask the whole frontier in one message, numbered, each with your recommended
answer:

    **Q1. <title>.** <the question, with its options>
    Recommended: <answer>, because <one line>.

Recommend the smallest answer that meets his goal. Recommend extra scope or
a safeguard only when the goal fails without it; offer anything else as an
option. Test a relationship between two things with a concrete case at its
edge.

A fact is never a question. Find it with a subagent (repo, docs, web) and
state it in the round with its source. Ask a taste question with two or three
small ASCII sketches rather than adjectives. When Saiful wants to see more, he
makes a claude.ai artifact or asks you for one. Keep its link so the plan
finds it: `mem save "<what it shows>: <url>" --title "artifact: <name>"`.

Done when the round holds every frontier decision with its recommended answer.

## 4. Record each answer as it settles

Challenge an answer once before you record it:

- He uses a term the glossary defines differently, or one word for two
  things: say so and ask which he means, with a precise term to adopt.
- He says how existing code behaves: a subagent checks it, and a
  contradiction goes back to him with its file:line.

    mem decide "<decision and why>" --by saiful --replaces "<your recommendation>"

Use `--replaces` only when he overrode your recommendation. A default you
took without asking is `--by agent`, so he can revisit it from the hub. When
a term's meaning is agreed, add it to the glossary in the same turn:

    mem wiki glossary --stdin --note "<term>: agreed in round <n>" < "$d/glossary.md"

Here `$d` is a scratch directory from `mktemp -d`. A glossary entry is the
term in bold, one or two sentences on what it is, and `_Avoid_:` with the
other words for it. It holds only this product's terms and no
implementation detail.

Done when every answer of the round is a mem decision and every agreed term
is on the glossary.

## 5. Repeat

Recompute the frontier and ask the next round. When he leaves mid-grill (no
reply, and the hub's `/api/presence` says `"watching": false`), post each open
question so he can answer from his phone, then end the session with a
handoff:

    mem ask --for human "<question>" --options "<a>,<b>" --recommend "<a>"
    mem handoff --set "Next: resume with the grill skill. Grilling <topic>: round <n> posted as <ids>."

When the frontier is empty, sweep for what the tree never held: failure and
empty states, limits as numbers, who may do what, data as it grows and
ages, data that exists already, outside services and their keys, and the
test seams (recommend the highest seam that exists). Each one gets a
decision, an agent default, or a question in one more round.

Done when the frontier is empty and the sweep left nothing open.

## 6. Confirm the summary

Write `grilling-summary`: the shared understanding in plain words, each point
naming the decision ids behind it, then the test seams, the fakes allowed and
what is out of scope. Ask him to confirm it. A change he asks for is a new
`mem decide` and a revised page.

Done when he confirmed the summary.

## 7. Write the spec

Write it from the decisions, the glossary and the research. Keep each section
under 2 KB, and split a page that passes 8 KB by subsystem.

- `spec` has these sections: problem, users and stories, the shape of the
  solution, decisions (one line each, linking the decision id), surfaces and
  Show paths (what Saiful opens and clicks for each feature), testing (the
  seams, the fakes allowed, the fast test command), out of scope, and an
  index of the `spec-<subsystem>` pages.
- `spec-<subsystem>` holds the detail of one subsystem.
- A section an artifact shaped links it as `[what it shows](<url>)`.
- `verify` holds launch, a read-only health check, how each surface is
  driven, and cleanup. The dogfooder keeps it current.

Write each page with `mem wiki <slug> --stdin --note "<what and why>"` and
list it on `index`. Then check the spec against the record and fix it in
place:

1. Each number, order and negative in a decision reads the same in the spec
   line that cites its id.
2. No line leaves a choice open ("or", "either", "TBD") or uses an
   adjective with no number ("fast", "robust").
3. Each rule lives in one section, and the others cite it.
4. Each Show path ends in something a person can see.

Then say the spec is ready for the plan skill.

Done when every decision appears in the spec's decisions section, every
feature has a Show path, the four checks pass, and `mem wiki lint` exits 0.
