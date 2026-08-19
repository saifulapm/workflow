# `mem --json` schemas

The committed contract for every verb's `--json` output (spec §14.15). One file
per output shape, named for the command line that produces it;
`tests/json_contract.rs` runs each of those command lines and validates what
comes back against the file next to it. If an output shape changes, the test
fails until the schema changes with it — that is the whole point.

JSON Schema draft 2020-12, restricted to what a small validator can check
without a new dependency: `type` (single or a list), `enum`, `required`,
`properties`, `items`, `additionalProperties: false`, and `$ref` to a sibling
file (`item.json` is the shared row shape; a schema may add its own `required`
alongside the `$ref`).

Rules that hold across the whole surface:

- **Envelopes are closed.** Every top-level document lists all its keys and sets
  `additionalProperties: false`. A new key is a contract change.
- **A filtered read that matches nothing prints its document with an empty array
  and exits 1** (spec §7). Callers for whom empty is a fine answer test the
  array, not the exit code: `mem log --kind ruling --type no-verifier --json`
  answering `{"items":[]}` means "no such ruling", not "the call failed".
- **Resolution failures print nothing on stdout.** An unknown id is exit 1, an
  ambiguous 8-character suffix exit 7, and the message goes to stderr — there is
  no half-document to parse. The exception is a verb that has already changed
  something: `mem prune --apply` given a mix of known and unknown ids archives
  the known ones, prints its `{"archived":[…]}` document, names the unknown ones
  on stderr and exits 1. That is the accept-then-report shape spec §4 already
  uses for an over-cap `mem status` (on disk, exit 6) — withholding the document
  would leave the caller unable to find out what landed.
- **Absent is `null`, never missing.** Optional item fields (`type`, `project`,
  `supersedes`, `superseded_by`, `answers`) are always present.
- **Hook envelopes are the runtime's shape, not mem's.** `hook-post-tool-batch`
  and `hook-stop` match what the Claude Code binary validates. PreCompact has no
  envelope at all: the hook is wired without `--hook-json`'s effect on output,
  because the runtime reads that hook's plain stdout as `newCustomInstructions`.
  `precompact.json` is the inspectable `--json` form of the same sentence, not
  the shape the runtime sees.
- **Some writes have no document.** `mem status --set`, `mem plan --set-file`,
  `mem plan --stdin` and `mem plan --clear` print nothing on success under
  `--json`; the exit code is the whole answer (0, or 5 for a CAS conflict, or 6
  for an over-cap status that still landed). They are the only verbs without a
  file here.
