# `hub`'s `/api/*` schemas

The committed contract for the three JSON endpoints (spec §3, AC3). One file
per document, named for the route that produces it, plus the three row shapes
they `$ref`. `tests/api.rs` runs the real endpoints against a real throwaway
store and validates what comes back against the file next to it — and, in the
other direction, checks that each schema rejects a document that has drifted, so
a schema nobody can fail is not mistaken for a contract.

The subset is mem's, deliberately, so the same small validator reads both:
`type` (single or a list), `enum`, `required`, `properties`, `items`,
`additionalProperties: false`, and `$ref` to a sibling file.

Rules that hold across all three:

- **Envelopes are closed** and carry exactly two keys: `degraded`, and the rows
  under the name the route is about (`questions`, `items`, `projects`). A new
  key is a contract change.
- **An empty result is an empty array with `degraded: null`.** mem exits 1 for a
  filtered read that matched nothing (spec §4a), and that is not an error; the
  only thing that sets `degraded` is mem being missing or printing something
  that will not parse.
- **Absent is `null`, never missing** — the same rule as mem's schemas. A
  project with no `status.md` has `"status": null`, which the page renders `—`.
- **Times are derived from the ULID `id`, not from mem's `created`.** Every mem
  list row reports `created` as a bare date, so it cannot order or age two items
  from the same day (review M-1). `asked_at`, `at` and `last_activity` are
  RFC 3339 with milliseconds; they are `null` only when an id is not a ULID.
