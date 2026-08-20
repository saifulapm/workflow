# workflow

Three binaries that run a solo development workflow, plus the thin surfaces
that carry it into an editor session.

- `mem/` is the system of record for project state: durable facts, rulings,
  logs, handoffs and blocking questions, kept as markdown outside every
  project repo and synced between machines.
- `workflow/` is the gate and the orchestrator: `verify`, `lint-msg`,
  `review-needed`, plan-driven `run`, `reap`, `park`/`resume`, `doctor`, and
  the body of the git hook stubs.
- `hub/` is a small web view over mem's question queue, served tailnet-only,
  so an open question can be answered from a phone.
- `skills/` holds the session-facing instructions (route, plan, implement,
  review, mem, unslop). `hooks/` holds the three git hook stubs. `adapters/`
  carry the same contract to other runtimes.

Install each binary with `cargo install --path <crate>`. Tests are
`cargo test` per crate plus `bash tests/run.sh`.
