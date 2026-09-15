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

The shipped rows are what is sensitive in any repository. What is load-bearing
in *this* one goes in beside them:

    mem project set review-paths "packages/core/** scripts/mutate.py"

Merged with the table, never replacing it.

## The read

    workflow read [--range <r>] [--against <text|wiki:slug>]

Run before the commit: the same reader the merge gate uses, over the working
tree or the range. Exit 0 (`ship`) means commit. Exit 1 (`fix`) means the
answer carries `[blocks]` findings — fix them, then read again. Two reads
with a `[blocks]` finding still open is not a third read: it is the
ruling-and-ask stop — `mem save --kind ruling` naming what is unresolved,
`mem ask` to the human. Exit 2 names no reader configured (`mem project set
review-model`); exit 3 is the deadline or a session that ended with no
verdict, naming the answer file to read by hand.
