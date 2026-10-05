import { atom, read, update } from 'claude-code'
import type { EngineInterface, Register, RenderElement, Timer } from 'claude-code'

import { bandLine } from './band'
import { commitsHere, refusal } from './guard'
import { asks, settleAnswers, waits } from './relay'
import type { AskedRow } from './relay'

// The switch is read from the environment, fixed when the session starts, so
// a session cannot turn the mods off for every other one with a command.
async function off($: EngineInterface): Promise<boolean> {
  return (await $.env.get('WORKFLOW_MODS')) === 'off'
}

// Whether a person is at the prompt, and whether the main loop is mid-turn.
const interactive = atom({ plugin: 'workflow', key: 'interactive' } as const, false)
const busy = atom({ plugin: 'workflow', key: 'busy' } as const, false)

// Whether the session ran `mem ask`, and the short ids of the answers it has
// already been handed or has read from a wait.
const asked = atom({ plugin: 'workflow', key: 'asked' } as const, false)
const delivered = atom({ plugin: 'workflow', key: 'delivered' } as const, [] as string[])

// The band's line and the minute timer that keeps it fresh between turns.
let line = ''
let tick: Timer | undefined

// Asks mem where the project stands. The line is cleared on anything but a
// clean exit with JSON that parses, a run that fails to start or times out
// included; that error still fails the hook, which the engine then skips.
async function refreshBand($: EngineInterface): Promise<void> {
  let next = ''
  try {
    const run = await $.process.run(['mem', 'context', '--brief', '--json'], {
      cwd: await $.session.cwd(),
      timeoutMs: 3000,
    })
    if (run.exitCode === 0) next = bandLine(JSON.parse(run.stdout))
  } finally {
    line = next
    $.ui.invalidate('ui.render')
  }
}

// The timer that asks mem again while a question is open between turns.
let watch: Timer | undefined

// The relay hands answers to a person's session only: an engine-started one
// reads its answers in its next brief, and a headless one has no prompt.
async function relays($: EngineInterface): Promise<boolean> {
  return (
    !(await off($)) &&
    (await read($, interactive)) &&
    (await $.env.get('WORKFLOW_ENGINE')) === undefined
  )
}

// Asks mem what this session asked and settles it: a new answer goes in as
// the person's prompt, unless `quiet`, when a wait has just printed it. The
// prompt is not awaited, since it runs once the session is idle.
async function checkAnswers($: EngineInterface, quiet: boolean): Promise<void> {
  if (!(await relays($)) || !(await read($, asked))) return
  const run = await $.process.run(
    ['mem', 'questions', '--asked-by', await $.session.id(), '--json'],
    { cwd: await $.session.cwd(), timeoutMs: 3000 },
  )
  // mem exits 1 when the session's list is empty.
  const rows: AskedRow[] = run.exitCode === 0 ? JSON.parse(run.stdout).questions : []
  const seen = new Set(await read($, delivered))
  const { submit, watch: open } = settleAnswers(rows, seen, quiet)
  await update($, delivered, () => [...seen])
  for (const row of submit) {
    void $.prompt.submit({ text: `Answer to #${row.short_id} (${row.title}): ${row.answer}`, asUser: true })
  }
  await $.ui.status(open.length > 0 ? `waiting on hub ${open.map(id => `#${id}`).join(' ')}` : undefined)
  if (open.length === 0) {
    watch?.cancel()
    watch = undefined
  } else if (watch === undefined) {
    watch = $.clock.every(20000, () => checkAnswers($, false))
  }
}

export const register: Register = on => {
  on('session.start', async ($, e, next) => {
    if (await off($)) return next(e)
    await update($, interactive, () => e.isInteractive)
    if (e.isInteractive) {
      // A /clear starts the session again; one timer is enough.
      tick?.cancel()
      tick = $.clock.every(60000, async () => {
        if (!(await read($, busy))) await refreshBand($)
      })
      await refreshBand($)
    }
    return next(e)
  })

  on('turn.start', async ($, e, next) => {
    if (await off($)) return next(e)
    // A prompt mid-turn would queue behind the turn; the turn's end asks again.
    watch?.cancel()
    watch = undefined
    await update($, busy, () => true)
    return next(e)
  })

  // A subagent's turn ends inside the main one, which is still running.
  on('turn.complete', async ($, e, next) => {
    if (await off($)) return next(e)
    if (e.agentId === undefined) {
      await update($, busy, () => false)
      // The relay goes first, so a failing band does not hold up an answer.
      await checkAnswers($, false)
      if (await read($, interactive)) await refreshBand($)
    }
    return next(e)
  })

  // Asks the binary whether the path is a new instruction file in a checkout
  // mem knows. Exit 0, an older binary's exit 2, a run that fails to start or
  // times out all let the write through; the pre-commit hook stays the gate.
  on('tool.call', { tool: ['Write', 'Edit', 'NotebookEdit'] }, async ($, e, next) => {
    if (await off($)) return next(e)
    const path = e.tool === 'NotebookEdit' ? e.notebook_path : e.file_path
    const run = await $.process.run(['workflow', 'hygiene', '--would-create', path], {
      cwd: await $.session.cwd(),
      timeoutMs: 3000,
    })
    if (run.exitCode !== 1) return next(e)
    $.ui.toast(`refused a new agent file: ${path}`)
    return { deny: refusal(run.stdout.trim()) }
  })

  // Notes a `mem ask` for the relay, and settles quietly after a wait that
  // printed its answers. Then asks the binary about the staged diff before a
  // commit in a checkout mem knows, so the session reads the finding before
  // git runs. Anything but exit 1 lets the commit through; the pre-commit hook
  // stays the gate, also for `git commit -a`, whose changes are staged only
  // as it runs.
  on('tool.call', { tool: 'Bash' }, async ($, e, next) => {
    if (await off($)) return next(e)
    if (asks(e.command)) await update($, asked, () => true)
    if (waits(e.command)) {
      const result = await next(e)
      if (result.deny === undefined && !result.isError) await checkAnswers($, true)
      return result
    }
    if (!commitsHere(e.command)) return next(e)
    const run = await $.process.run(['workflow', 'hygiene', '--staged', '--known'], {
      cwd: await $.session.cwd(),
      timeoutMs: 5000,
    })
    const finding = run.stdout.split('\n').find(l => l.includes(' hard '))
    if (run.exitCode !== 1 || finding === undefined) return next(e)
    $.ui.toast(`refused a commit: ${finding}`)
    return { deny: refusal(finding) }
  })

  on('ui.render', { component: 'AbovePrompt' }, async ($, e, next) => {
    if ((await off($)) || e.props.hasSurvey || line === '') return next(e)
    const { Box, Text } = $.ui.resolve(e)
    // This module is .ts, so it calls h itself; JSX would type the tree so.
    return h(
      Box,
      { flexDirection: 'column' },
      h(Text, { dimColor: true }, line.slice(0, e.props.bodyColumns)),
      await next(e),
    ) as RenderElement
  })
}
