---
name: review
description: Use before finishing a change that touches auth, payments, secrets, migrations, jobs, manifests, deploy config or a public API.
---

# review

## Does this change want one?

    workflow review-needed [--diff <range>]

Exit 0 means yes. The change set is the working tree plus the range, because an
untracked `.env` or a brand-new guard file is invisible to a diff. The table is
matched case-insensitively: on a Laravel layout the interesting files are
`app/Http/Middleware/Authenticate.php`, `app/Policies/`, `PrivateKey.pem` —
a case-sensitive table saw none of them.

Do not argue with exit 0. It is the disqualifier for the one-shot lane too.

## Two cold reviewers

Cold means they read the diff and the spec, and nothing else: not your
reasoning, not each other's findings, not a summary of what you were trying to
do. Give each one the range and the requirement, and one lens:

1. **Reproduce the defect.** Find a concrete input or state where this code
   does the wrong thing. Failure scenario or nothing.
2. **Spec compliance.** Does it do what was asked, all of it, and nothing that
   was not asked?

Each answers in 400 words or less. Do not merge their reports, do not
pre-judge, do not tell reviewer two what reviewer one said. Two independent
readings are the point; one averaged reading is worth less than either.

Dispatch depends on the runtime: an Agent tool call with the model named
explicitly, or `claude -p --agent`, or — where neither exists — say so and
record a ruling rather than quietly skipping the review.

## The fix loop, bounded

- Attempts 1–3: same reviewers, they check their own findings.
- Attempts 4–5: fresh reviewers. If three tries did not close it, the reading
  is stuck, not the code.
- Still open after five: stop. Record what is unresolved as a ruling —
  *what — why — cost if wrong* — and take it to the human.

## What a finding must carry

A file and line, a concrete failure scenario, and what the correct behaviour
would be. "Consider extracting this" is not a finding; it is a preference, and
preferences do not block. Correctness and requirement gaps block.
