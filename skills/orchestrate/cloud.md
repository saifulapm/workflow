# Cloud run

A cloud run is the orchestrate skill in a claude.ai cloud session
(`CLAUDE_CODE_REMOTE_SESSION_ID` is set), started from its prompt. The
prompt names the milestone, its branch `cloud/<project>-<milestone>` and the
branch to cut it from. Everything in the skill holds, with these changes:

- First check out the cloud branch as the prompt says, never `main`: mem
  here reads the same store the devices read, and it tells this project
  from others in the same repo by that branch. Then
  record the run: `mem project set runner cloud <id> --milestone <slug>`,
  where `<id>` is `$CLAUDE_CODE_REMOTE_SESSION_ID` with `cse_` swapped for
  `session_`.
- The trunk is the cloud branch. Land each task on it as §2 says, then `git push origin
  cloud/<project>-<milestone>` right after its tick, so the branch gains one
  commit, or one fast-forward, per landed task.
- Before every push, run `mem share export` and commit the
  `.mem-outbox.json` it stages, when it stages one: no signal reaches a
  session that ends, so writes the store has not taken yet travel on the
  branch, and the next machine to read the project sends them.
- A task that needs a device, Saiful's desktop, or a login the cloud cannot
  reach is local-only, and its checklist line ends `[local]` (mark it when
  you cut the checklist). Land every task before it that does not wait on
  it, commit nothing for it, push the branch, and write a handoff starting
  `parked: local-only <task>:` that names it and says what it needs. File
  no question: `workflow go` on a device reads that handoff, takes the
  branch and runs the rest. Then stop.
- The dogfood walk drives here only what the dogfood skill says a cloud
  session can. When its report names `local-only` steps, the walk is a
  local-only task: land and push the rest, and park with `parked:
  local-only dogfood <steps>:` as above, so a device walks them.
- A question only Saiful can answer never parks a cloud run: nothing can
  wake a stopped cloud session. Ask with `mem ask --for saiful`, do what
  does not depend on the answer, then wait in the session with `mem
  questions --wait <id>`, run again each time it returns without one. The
  answer reaches this session's local hub over Tailscale.
- Land as §7 says, except that nothing runs `workflow go` and nothing
  pushes `main`, and the branch is pushed before `mem roadmap --tick`: the
  tick tells the mini it may fast-forward, so the review and dogfood fixes
  must be on the branch by then. The run ends with the branch pushed, the
  milestone ticked and the handoff written. The mini fast-forwards its trunk from the branch.
- After the last handoff, parked or landed, run `mem share export` once
  more, commit what it stages and push: the tick and the handoff come after
  the last task's push, and a store cut off at the end would keep them.
- Stop after 6 hours with a handoff starting `parked: 6-hour stop`. There
  is no amx here, so end by saying so.
