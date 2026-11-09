# Cloud run

A cloud run is the orchestrate skill in a thread of a project's Claude
Project on claude.ai (`CLAUDE_CODE_REMOTE_SESSION_ID` is set). The
Project's coordinator starts the thread on the start line, `Run the next
milestone of <project>.`; the Project instructions have the thread run
`workflow go <project> --cloud --print`, which claims the run in mem and
prints the prompt. The prompt names the milestone, its branch
`cloud/<project>-<milestone>` and the branch to cut it from. Everything in
the skill holds, with these changes:

- The run is already recorded as this session's: `workflow go --cloud
  --print` did it, and refused if another run held the project. Check out
  the milestone branch as the prompt says, never build on `main`: mem
  here reads the same store the devices read.
- After a pause between turns the machine may have rebooted: the disk is
  kept, but each shell starts in /home/user. `cd` into the checkout the
  prompt names before any git or mem command.
- The trunk for every task is the milestone branch. Land each task on it
  as §2 says, then `git push origin cloud/<project>-<milestone>` right
  after its tick, so the branch gains one commit, or one fast-forward, per
  landed task. Work in progress goes on that branch too, never on main,
  including what Claude Code's own stop hook asks to commit and push.
- Before every push, run `mem share export` and commit the
  `.mem-outbox.json` it stages, when it stages one: no signal reaches a
  session that ends, so writes the store has not taken yet travel on the
  branch, and the next machine to read the project sends them.
- A task that needs a device, Saiful's desktop, or a login the cloud cannot
  reach is local-only, and its checklist line ends `[local]`: the device
  work the plan's `Runs on: cloud, then device:` line names, cut as the
  last tasks. Land every other task, then run the review (§5) and the
  dogfood walk (§6) here, the walk driving only what the dogfood skill says
  a cloud session can, and land their fixes. Push the branch and write a
  handoff starting `parked: local-only <task>: review and walk done at
  <sha>; left:` that names the `[local]` tasks, the walk steps the dogfood
  report marked `local-only`, and what they need. File no question: a
  device's `workflow go` reads that handoff, takes the branch and runs
  what is left. End the report with `parked: local-only <task>`.
- A device need the plan did not name, in a `Runs on: cloud` milestone or a
  step its line left out, parks the same way and gets `mem finding add
  --milestone <slug> --step plan` naming it, so the next plan names it.
- A question only Saiful can answer never parks a cloud run: a thread
  that stopped wakes only when someone types in it. Ask with `mem ask
  --for saiful`, do what does not depend on the answer, then wait in the
  thread with `mem questions --wait <id>`, run again each time it returns
  without one. The answer reaches this session's local hub over Tailscale.
- Land as §7 says, with these changes. Write the handoff first, starting
  `Next: <the next milestone>. <milestone> landed on main.`, then export
  and commit the outbox, then run `workflow land`. It rebases the branch
  on origin/main, runs the test command, pushes main as a fast-forward and
  never forced, ticks the milestone, carries the writes the store has not
  taken, and prints the report's last line. Never push main, never tick
  the roadmap and never run `workflow go` yourself: the coordinator starts
  the next thread from that line.
  - Tests red after the rebase: fix them on the branch test-first and run
    `workflow land` again. After two failed fixes, hand off `parked: tests
    red after rebasing on main: <what fails>` and push the branch.
  - `parked: trunk moved: ...`: hand off with that line and push the
    branch.
- After a landing, as the very last command, log the machine off the
  tailnet as the prompt says; not when parked.
- When usage limits reset and the thread continues, carry on from `mem
  plan` and the handoff; never start the milestone over.
- Stop after 6 hours with a handoff starting `parked: 6-hour stop`, an
  export and a push of the branch. There is no amx here, so end by saying
  so.
- The report's last line is exactly one of `next: Run the next milestone
  of <project>.`, `done: <project>'s roadmap is done`, or `parked: <why>`,
  with nothing after it: the coordinator reads only that line.
