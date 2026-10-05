import { expect, mock, test } from 'claude-code/testing'
import type { EventName, On } from 'claude-code'

// Every call a hook can make on `$` but env.get, which the switch itself reads.
const CALLS = [
  'turn.abort', 'prompt.read', 'tool.list', 'tool.register', 'command.list',
  'command.register', 'config.list', 'agent.list', 'agent.register',
  'ui.toast', 'ui.status', 'ui.log', 'ui.notice', 'ui.invalidate', 'ui.open',
  'ui.close', 'ui.panes', 'ui.copy', 'ui.blit', 'fs.read', 'fs.write',
  'fs.list', 'fs.exists', 'fs.stat', 'fs.ancestors', 'store.get', 'store.set',
  'store.delete', 'store.keys', 'state.get', 'state.set', 'clock.now',
  'clock.sleep', 'clock.after', 'clock.every', 'http.fetch', 'process.run',
  'settings.read', 'env.set', 'model.complete',
  'model.classify', 'model.fork', 'audio.play', 'audio.speak', 'mcp.call',
  'mcp.connect', 'session.cwd', 'session.root', 'session.model',
  'session.turns', 'session.id', 'session.messages', 'session.repo',
  'session.surface', 'session.surfaces', 'session.authorize',
  'session.usage', 'session.version',
] as const

// The engine's side of the three events the base hooks: each echoes what core does.
function engine(on: On) {
  on('session.start', (_$, e) => ({ cwd: e.cwd }))
  on('turn.start', (_$, e) => ({ turnId: e.turnId }))
  on('turn.complete', (_$, e) => ({ text: e.answer }))
}

const done = { answer: 'done', durationMs: 1, isAborted: false, reason: 'answer' } as const

test('with WORKFLOW_MODS=off no hook calls anything but env.get', async ($, on) => {
  mock.env(on, { WORKFLOW_MODS: 'off' })
  const calls: string[] = []
  const record = (name: string) => {
    calls.push(name)
    throw new Error(`${name} called with the mods off`)
  }
  for (const name of CALLS) {
    ;(on as (event: EventName, hook: () => unknown) => void)(name, () => record(name))
  }
  // process.spawn streams, so its hook is a generator.
  on('process.spawn', async function* () {
    record('process.spawn')
  })
  engine(on)

  await $.session.start({ cwd: '/', surface: 'terminal', isInteractive: true })
  await $.turn.start({ text: 'hi', turnId: 't1' })
  await $.turn.complete({ ...done, turnId: 't1' })

  expect(calls).toEqual([])
})

test('a subagent ending its turn leaves the session busy', async ($, on) => {
  mock.env(on, {})
  engine(on)
  const writes: string[] = []
  on('state.set', async (_$, e, next) => {
    writes.push(`${e.key}=${e.value}`)
    return next(e)
  })

  await $.session.start({ cwd: '/', surface: 'terminal', isInteractive: true })
  await $.turn.start({ text: 'hi', turnId: 't1' })
  await $.turn.complete({ ...done, turnId: 't1', agentId: 'a1' })
  expect(writes).toEqual(['interactive=true', 'busy=true'])

  await $.turn.complete({ ...done, turnId: 't1' })
  expect(writes).toEqual(['interactive=true', 'busy=true', 'busy=false'])
})
