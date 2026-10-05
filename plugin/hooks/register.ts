import { atom, read, update } from 'claude-code'
import type { EngineInterface, Register, RenderElement, Timer } from 'claude-code'

import { bandLine } from './band'

// The switch is read from the environment, fixed when the session starts, so
// a session cannot turn the mods off for every other one with a command.
async function off($: EngineInterface): Promise<boolean> {
  return (await $.env.get('WORKFLOW_MODS')) === 'off'
}

// Whether a person is at the prompt, and whether the main loop is mid-turn.
const interactive = atom({ plugin: 'workflow', key: 'interactive' } as const, false)
const busy = atom({ plugin: 'workflow', key: 'busy' } as const, false)

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
    await update($, busy, () => true)
    return next(e)
  })

  // A subagent's turn ends inside the main one, which is still running.
  on('turn.complete', async ($, e, next) => {
    if (await off($)) return next(e)
    if (e.agentId === undefined) {
      await update($, busy, () => false)
      if (await read($, interactive)) await refreshBand($)
    }
    return next(e)
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
