import { atom, update } from 'claude-code'
import type { EngineInterface, Register } from 'claude-code'

// The switch is read from the environment, fixed when the session starts, so
// a session cannot turn the mods off for every other one with a command.
async function off($: EngineInterface): Promise<boolean> {
  return (await $.env.get('WORKFLOW_MODS')) === 'off'
}

// Whether a person is at the prompt, and whether the main loop is mid-turn.
const interactive = atom({ plugin: 'workflow', key: 'interactive' } as const, false)
const busy = atom({ plugin: 'workflow', key: 'busy' } as const, false)

export const register: Register = on => {
  on('session.start', async ($, e, next) => {
    if (await off($)) return next(e)
    await update($, interactive, () => e.isInteractive)
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
    if (e.agentId === undefined) await update($, busy, () => false)
    return next(e)
  })
}
