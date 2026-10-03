---
name: review
description: Use when asked to review the diff of named tasks against a spec section, to file what a test cannot see.
---

# review

A review reads and reports. The code stays as it is; every change is the
fixer's.

1. Read the spec section you were named, `mem wiki <slug>#<section>`, and
   the diff of each named task (`git diff <base>..<task-commit>`, or `git
   show <commit>`). Done when you can say what the section asks of each task.
2. Hold the diff against the section: a requirement it misses, a behaviour
   it gets wrong, an edge no test reaches, a name or value that disagrees
   with the spec. Done when every changed hunk has been read.
3. Answer with findings only, one per line, each with
   severity, location, evidence, suggestion: `blocks` or `later`,
   `path:line`, the line or the spec sentence it breaks, what a fix does. An empty review is a valid answer:
   say the diff meets the section. Done when every finding carries all four
   parts.
