# Cloud plan

A cloud plan is the plan skill in a claude.ai cloud session
(`CLAUDE_CODE_REMOTE_SESSION_ID` is set), started by `workflow plan <project>
--cloud` from its prompt. Everything in the skill holds, with these changes:

- First check out the branch `cloud/<project>-plan` as the prompt says,
  never `main`: mem here reads the same store the devices read, and it tells
  this project from others in the same repo by that branch. Then record the
  run: `mem project set runner cloud <id> --milestone plan`, where `<id>` is
  `$CLAUDE_CODE_REMOTE_SESSION_ID` with `cse_` swapped for `session_`.
- A plan changes no file of the repository. Write in a scratch directory and
  store everything with mem, as §4 and §5 say.
- Saiful is never in this chat. Ask each fork of §2 and the roadmap review
  of §5 with `mem ask --for saiful`; the hub shows each with its buttons.
  Nothing can wake a stopped cloud session, so never stop to wait: do what
  does not depend on the answer, then run `mem questions --wait <id>` again
  each time it returns without one.
- `npx ctx7@latest` works here: the environment carries `CONTEXT7_API_KEY`
  and full network access.
- §6 changes: when Saiful approves, set `mem roadmap --status approved` and
  start nothing. There is no amx here, and `workflow go` never runs in the
  cloud. Write a handoff whose first line is `Next: workflow go <project>
  --cloud`, so the hub's Run in cloud or the mini starts the first milestone,
  cut from this branch.
- Then run `mem share export`, commit the `.mem-outbox.json` it stages when
  it stages one, and `git push origin cloud/<project>-plan`, even with no new
  commit: no signal reaches a session that ends, so writes the store has not
  taken yet travel on the branch, and the first cloud milestone imports them.
- Never push `main`. Stop after 6 hours with a handoff starting `parked:
  6-hour stop`, then that export and push.
