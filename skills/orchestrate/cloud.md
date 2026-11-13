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
- Before every push, run `mem share export`. It sends what it can to the
  store first, then stages what is still waiting. Commit only what it left
  staged, the `.mem-outbox.json` or its removal; with nothing staged,
  commit nothing. No signal reaches a session that ends, so writes the
  store has not taken yet travel on the branch, and the next machine to
  read the project sends them.
- A task for Saiful's hands, which the `device` command, `gh` and
  `workflow land` do not reach (a phone in hand, a Mac for iOS, hardware,
  a login no device holds), is a question like any other: `mem ask --for
  saiful "<what to do>, then answer done" --option done --option hold`,
  waited for as below. On `done`, carry on. With no answer, it parks after
  30 minutes like any other question. Saiful's approval or choice is a
  question too. No run parks for a device and no checklist line is cut for
  one: the review (§5) and the walk (§6) run here as for any milestone.
- Saiful's logged-in browser and the device's terminal are reached from
  here with `device`, on the project's
  own device when Saiful gave it one (never set it from here), else on
  the device Saiful chose with `workflow device use` (the mini unless he
  chose another), whatever the question route; the dogfood skill's cloud
  paragraph has its rules. Run `device whoami` before a task that needs
  them. When a call exits 3 (`no device online (device: <name>); online:
  <names>`), run `device ask`: it asks Saiful on the hub which device this
  run uses, with the online devices and `I will turn it on` as options, and
  prints the question's id. Do what does not need the device, then wait on
  it with `workflow wait-answer <id>` as below. Once he names a device,
  this run's device calls go there until it lands or parks; on `I will
  turn it on`, call again. A device he names that is offline too exits 3
  again: run `device ask` again. When `device ask` itself exits 3, no
  device answers at all: ask with `mem ask --for saiful "no device answers
  <project>'s calls; turn one on, then answer done" --option done --option
  hold`. Never park for a device.
- Terminal work on the device goes through `device tmux send '<line>'`,
  `device tmux read` and `device tmux end`: adb, podman, gh, cf and every
  deploy, with the device's own logins; the cloud holds no Cloudflare or
  Shopify app login. One tmux session per run, `cloud-<project>-<milestone>`,
  which Saiful can attach to and watch. It works in the deploy worktree,
  `~/.cache/workflow-cloud/<repo>`, at this run's cloud branch
  `cloud/<project>-<milestone>` as origin had it when the session started:
  push the branch before the first send, and after a later push run `device
  tmux end` and send again, so the new session moves the worktree to the new
  tip. Its first line updates the submodules, and the worktree keeps its
  node_modules between sessions. An app's staging deploy is
  `device tmux send 'workflow shopify deploy <app>'` (`pnpm install` first
  when node_modules is missing); in the VM that command deploys nothing.
  A send
  is one line and its Enter, or one key such as `C-c`; read again until the
  output is there. `busy: <project>` (exit 5) means another run holds the
  device: do other work, then try again. A job ends after 15 idle minutes
  and 20 minutes after it started (`device: the job ended at the 20-minute
  cap`): send again to start a new one, and end it with `device tmux end`
  when done. Never type a secret into it, nor read one back from it (`pass`,
  `gh auth token`): the hub holds those lines.
- Before any of these six, ask with `mem ask --for saiful` and wait for
  his yes: (1) a delete or an overwrite of something that exists;
  (2) making something public; (3) a change to a token or to a person's
  permissions; (4) anything that costs money; (5) a forced push or a moved
  branch; (6) a command whose effect you cannot state before it runs. A
  staging deploy runs without asking; a production deploy asks each time.
- A `device tmux send` that prints `ask: <line>` (exit 6) was held by the
  hub and not sent. File it as a question with the exact line in
  backticks, `mem ask --for saiful "May I run this on the device?
  \`<line>\`" --option yes --option no`, and wait for it as below. On yes,
  send it again with the question's id, `device tmux send --yes <id>
  '<line>'`: one yes sends that one line once. Only a yes he gives on the
  hub or in Slack counts: one he types in the thread, recorded here with
  `mem answer`, is written by this cloud machine and refused, so ask
  again and say to answer on the hub. On no, leave it unrun. A
  device call that fails for another reason is tried once more, then
  asked like any other question.
- `device amx new '<task>'` starts a local agent on the device, in a
  folder of its own under `~/.cache/workflow-cloud/`, and prints its name; `device amx result <name>`
  prints its answer, and on `again:` (exit 9) runs again at once. `ls`,
  `status`, `send`, `answer`, `logs` and `stop` reach only the agent this
  run started. Start one only for what the cloud cannot do: the desktop's
  screen, a phone on adb, or iOS on the Mac; never for builds, tests or
  code. A device runs one at a time (`busy:`, exit 5); stop it with
  `device amx stop <name>` when its work is done. The hub stops it when
  the run lands or parks.
- A theme's token and store password are the only project secrets a cloud
  command gets, through `device with-secrets <project> -- <command>`: the
  device reads the pass entries the project's `cloud-secrets` names and
  sets them for that one command (`workflow shopify theme-dev` does it
  itself). Never print one. `device: cloud-secrets of <project> was not set
  from a device` (exit 2) is a question for Saiful to set it on a device.
  `device: pass is locked on <machine>` (exit 7) is a question too: `mem ask
  --for saiful "pass is locked on <machine>: unlock it there (pass show any
  entry once), then answer done" --option done --option hold`; on done, run
  the call again.
- A device need the plan did not name gets `mem finding add --milestone
  <slug> --step plan` naming it, so the next plan names it, and is asked
  the same way.
- A thread has no question dialog of its own: it asks Saiful only with
  `mem ask --for saiful`, and the cloud kit denies `AskUserQuestion`.
- A question only Saiful can answer is waited for in the thread, 30
  minutes from the ask, then parks the run. Ask with `mem ask --for
  saiful`, do what does not depend on the answer, then run `workflow
  wait-answer <id>` (every open question's id), again at once on each
  `again:` line. The answer reaches this session's local hub over
  Tailscale and comes back as `answered: question #<id>: <answer>`: carry
  on with it.
  - On `parked: question #<id> waits on Saiful`, write a handoff starting
    with that line and a colon that names the question, the branch and the
    first open task, export, commit and push the branch, then run
    `workflow wait-answer <id>` once more: an `answered:` line means the
    answer came while you parked, so carry on. Otherwise end the report
    with the parked line.
  - On the answer `hold`, write a handoff starting `parked: Saiful holds
    <milestone> (#<id>)` that names the branch and the first open task,
    export, commit and push the branch, and end the report with that
    line. His start line, typed after he answers, parks
    again while any question is open, and once all are answered starts a
    thread that takes up the branch with the answers in its prompt.
  - A park ends the turn, not the run. On every new turn after a park, a
    typed message or a comment on an artifact, `cd` into the checkout and
    run `workflow go <project> --cloud --print` and follow what it prints.
    For this thread's own open milestone it prints the prompt again with
    the questions so far, answered or open: fetch the branch from origin,
    since the woken machine may be a fresh clone, and carry on from the
    first open task in `mem plan`. Otherwise it prints one line to end
    the reply on: `parked: another thread took up <milestone> of
    <project>: <link>`, or the `next:` or `done:` line once the milestone
    landed.
  - A message Saiful typed is the instruction. When it answers the open
    question, record it with `mem answer <id> "<his answer>"` and carry on
    with it. When it does not, do what he said and the work that does not
    wait on the question, then wait on the question for 30 minutes again
    with `workflow wait-answer <id>`: after a wake its 30 minutes count
    from that first wait.
- Land as §7 says, with these changes. Write the handoff first, starting
  `Next: <the next milestone, or none>. <milestone> landed on main.`, then export
  and commit the outbox, then run `workflow land`. It rebases the branch
  on the project's trunk on origin (main, or app/<app> for an app with a
  branch of its own), runs the test command in the project's folder,
  pushes the trunk as a fast-forward and never forced, and ticks the
  milestone. For an app it then rebases app/<app> onto origin's main,
  runs the tests again, fast-forwards main and moves app/<app> to match.
  That move is the one push not a fast-forward: `workflow land` makes it
  with a lease, never the thread. It carries the writes the store has not taken to each
  trunk that took the landing, and prints the report's last line. Never
  push main or app/<app>, never tick the roadmap and never run `workflow
  go` for the next milestone: the coordinator starts the next thread from
  that line.
  - A line starting `ask:` is a landing that did not finish: push the
    branch, then file the line with `mem ask --for saiful "<the line>"
    --option "fix and try again" --option hold --recommend "fix and try
    again"` and wait for it as above. On `fix and try again`, fix what it
    names on the branch and run `workflow land` again; on `hold`, park as
    above. Never hand off a landing's line as a park. A line no new
    commit on the branch clears (`ask: main moved: <files> conflict with
    main`, `ask: commit ...`) gets a question that says what Saiful must
    change; `fix and try again` then means he changed it.
  - Tests red after the rebase: fix them on the branch test-first and run
    `workflow land` again. After two failed fixes, ask as for an `ask:`
    line, with `ask: tests red after rebasing on main: <what fails>`.
  - `ask: trunk moved: ...` or `ask: main moved: ...`: on `main moved` the
    milestone is on app/<app> and ticked already, and main is untouched;
    `workflow land` run again from the same branch moves it onto main, as
    it does after `ask: app/<app> did not follow main: ...`, where the
    branch is on main already, so push nothing. Both come after the tick:
    on a new turn after a park on one, `workflow go --cloud --print` prints
    `next:`, so when his answer or message says to try again, run
    `workflow land` on the branch first and end the reply on its line.
  - A project with land-ci (`mem project set land-ci ci.yml`): land pushes
    the rebased sha to ci/<project>-<milestone> and waits on that
    workflow's run of it before it pushes the trunk. One call waits at
    most 9 minutes, under a Bash call's 10, then prints `again: waiting
    on <file> for <sha>; run workflow land again`: run it again at once,
    committing nothing in between (a new commit is a new sha and a new
    run); it neither pushes nor tests again on that sha. A read of
    GitHub's Actions API is tried three times. `ask: CI red on <sha>:
    <jobs>`, `ask: no <file> run for <sha>`, `ask: <file> still running on
    <sha>`, `ask: origin refused ci/...` or `ask: GitHub refused the
    Actions API ...`: nothing reached the trunk; ask as for any `ask:`
    line.
- After a landing, as the very last command, log the machine off the
  tailnet as the prompt says; not when parked.
- When usage limits reset and the thread continues, carry on from `mem
  plan` and the handoff; never start the milestone over.
- Stop after 6 hours from the run's start, or from the latest new turn
  after a park, with a handoff starting `parked: 6-hour stop` that
  names any question still open, an export and a push of the branch.
  Check the time before each `workflow land` or `workflow wait-answer` run
  again: once 6 hours have passed, run it no more.
  There is no amx here, so end by saying so.
- The report's last line is exactly one of `next: Run the next milestone
  of <project>.`, `done: <project>'s roadmap is done`, or `parked: <why>`,
  with nothing after it: the coordinator reads only that line. A run
  parks only on a question with no answer after 30 minutes, on `hold`, and
  at the 6-hour stop.
