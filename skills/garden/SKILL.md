---
name: garden
description: Use to keep a project's wiki true: lint, then the pages touched since the last pass, for dead links, oversize pages, superseded pages and contradictions.
---

# garden

A garden pass leaves the wiki as true as it found it or truer, and every page
still there. A page that is done becomes a stub; a deletion returns on the
next sync.

## 1. Lint

    mem wiki lint

Fix each oversize, orphan and unindexed page it names, by the rules in the
steps below: an orphan or unindexed page gets its line in `index`.

Done when `mem wiki lint` exits 0.

## 2. Find the pages touched since the last pass

    mem log --type garden --limit 1    # when the last pass ended
    mem wiki                           # every page with its modified time

Read `index` whole, then each page modified after the last pass. With no
earlier pass, read every page.

Done when you hold the list of touched pages and have read each one.

## 3. Fix dead links

A link is `[name](name.md)` and lands on a page `mem wiki` lists. Point a dead
link at the page that now holds what it meant, or cut the link and keep the
words.

Done when every link in `index` and in each touched page names a listed page.

## 4. Split pages over 8 KB

Split by section: move whole `## ` sections into a new page, leave a one-line
pointer to it where they stood, and add the new page's line to `index`. A
section over 2 KB splits the same way.

Done when `mem wiki` shows no page over 8 KB.

## 5. Stub superseded pages

A page whose subject another page now holds becomes a one-line stub naming its
replacement, `Superseded by [name](name.md).`, and its line leaves `index`.

Done when each superseded page is a stub and `index` lists only live pages.

## 6. Note contradictions

Two pages that disagree on a fact are the owner's to settle: write one line
naming both sections and the fact.

    mem log "contradiction: <slug>#<section> says <x>, <slug>#<section> says <y>"

Leave both pages as they are.

Done when every disagreement you found has its log line.

## 7. Close the pass

Every write is `mem wiki <slug> --stdin --note "<what changed and why>"`, or
`mem wiki <slug>#<section> --stdin --note` for one section, so the note is the
page's history. Then mark the pass:

    mem log --type garden "garden: <pages read>, <what changed>"

Done when the garden line is written and `mem wiki lint` still exits 0.
