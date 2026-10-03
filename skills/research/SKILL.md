---
name: research
description: Use before a project is planned to study its competitors, resources and ideas, and write what matters into the wiki as research pages and a summary.
---

# research

Research turns a brief into four wiki pages the grilling starts from. Sonnet
subagents do the bulk reads; you keep the judgment: what matters, what is
false as designed, where the opportunities are, what only the owner can
answer.

## 1. State the questions

Read `mem brief`, `mem wiki research-summary` if it exists, and the pages
`mem wiki` lists. Write down the three questions this research must answer,
each one a sentence a source can settle or refute.

Done when three questions are stated and no existing page already answers one.

## 2. Fan out

Start one Sonnet subagent per source family, all in one message so they run
at once:

- competitors: the products that solve this problem today, from the web;
- resources: the APIs, libraries, services and data the product could stand
  on, from docs and pricing pages, and `npx ctx7@latest` for libraries;
- the repo: what the existing code already does, when there is one.

Give each the three questions and ask for facts, each with the URL or path it
came from. Save every source a fact rests on as it was taken:

    mem raw add <url|file>

A later session re-reads `raw/` instead of re-fetching, and a page that has
moved on since still shows what it said.

Done when every family has reported and every source a fact cites is in
`raw/`.

## 3. Write the research pages

One section per subject, each under 2 KB, each ending with its sources:

- `research-competitors`: one section per competitor: what it does, its
  price, what it does badly, what to copy.
- `research-resources`: one section per API, library, service or data
  source: what it gives, its price, its limits.
- `research-ideas`: one section per idea, with the evidence for it.

Write each page the moment it is complete, and list it in `index`:

    mem wiki <slug> --stdin --note "<what it covers and why>" <page.md

A usage-limit pause then loses nothing, and the owner reads the pages on the
hub as they land.

Done when `mem wiki <slug> --sections` shows every section of all three pages
under 2 KB and every section names a source.

## 4. Write the summary

Write `research-summary` last, as one page with four sections:

- What matters: the few facts that shape the product.
- False as designed: what the brief assumes that the sources show is
  impossible, unpriced, forbidden or already tried and failed. The owner
  values this section most; give each item the evidence that kills it.
- Three opportunities: the three biggest, each with the section that backs
  it.
- Questions for the owner: what only the owner can decide, each naming the
  research section it comes from as `wiki:<page>#<section>`.

Done when every owner question names its section, every claim traces to a
research section, and `mem wiki lint` exits 0.

## Facts and judgment

A subagent reports facts with sources; a fact without a source stays out of
the pages. Judgment lives only in `research-summary` and is yours. When two
sources disagree, the section says so and names both.
