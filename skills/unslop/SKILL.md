---
name: unslop
description: Use when writing prose a human will read, such as a README, docs, a PR body, a product artifact or a long commit body, to cut machine tells and keep a human voice.
---

# unslop

`workflow lint-msg` catches the mechanical layer in commits and PR bodies:
the vocabulary, em dashes, curly quotes, filler phrases. This is the judgment
layer, applied while writing, anywhere prose lands.

## Voice

Sterile writing is as much a tell as any banned word. Have opinions; react to
facts instead of listing pros and cons. Vary rhythm: short sentences, then
longer ones that take their time. Say "I" when it fits. Let some mess in,
because perfect structure looks machine-made. Be specific: not "this is
concerning" but the concrete thing that concerns you.

## Content tells

- Puffery. "pivotal moment", "testament to". State what happened.
- Superficial participles. "highlighting...", "ensuring...", "reflecting...".
  Delete, or expand into a real claim with a source.
- Vague attribution. "Experts believe", "some argue". Name the source or cut.
- Formulaic shapes. "Despite challenges... continues to thrive", "not just X
  but Y", forced groups of three, fake "from X to Y" ranges. Say the point.
- Synonym cycling. Pick one name for a thing and repeat it.

## Style tells

- No em or en dashes, and no parentheses standing in for them. End the
  sentence or use a comma. Straight quotes only.
- Colons before a list or an example only, never as mid-sentence glue.
- Sentence case headings. No decorative emoji. Not every noun in bold.
- An inline-header list that restates its own label ("**Performance:**
  performance improved...") becomes prose.

## Plain speech

- Say what it does, not how it feels. If a sentence could sit unchanged in
  another project's docs, it says nothing about this one. Cut it, or replace
  it with the mechanism or a number.
- The plain word over the fancy one: use, not utilize or leverage; help, not
  facilitate; is, not "serves as" or "boasts".
- Active voice, with the actor named: "the compiler validates queries", not
  "queries are validated". Passive only when the actor is unknown or truly
  does not matter.
- Cut adverbs; use a stronger verb or the measured number instead.
- One idea per sentence. If the reader has to backtrack, split it.
- "In order to" is "to". "It is important to note that" is nothing.
- Abstract metaphor nouns (substrate, wedge, north star, flywheel, paradigm)
  have a concrete word. Use it.

## Filler and chat

Delete chatbot phrases ("I hope this helps", "Let me know if"), hedging
stacks ("could potentially possibly"), and generic conclusions ("the future
looks bright"). State plans and facts, or end.

## Last pass

Ask "what makes this obviously machine written?" and fix what you find.
