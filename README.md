# workflow

Three binaries that run a solo development workflow, plus the thin surfaces
that carry it into an editor session.

- `mem/` — the system of record for project state: durable facts, rulings,
  logs, handoffs and blocking questions, kept as markdown outside every
  project repo and synced between machines.
- `workflow/` — the gate and the orchestrator: `verify`, `lint-msg`,
  `review-needed`, plan-driven `run`, `reap`, `park`/`resume`, `doctor`, and
  the body of the git hook stubs.
- `hub/` — a small web view over mem's question queue, served tailnet-only,
  so an open question can be answered from a phone.
- `skills/` — the session-facing instructions (route, plan, implement,
  review, mem). `hooks/` — the three git hook stubs. `adapters/` — the same
  contract for other runtimes.

Install each binary with `cargo install --path <crate>`. Tests are
`cargo test` per crate plus `bash tests/run.sh`.
