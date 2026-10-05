import { expect, mock, test } from 'claude-code/testing'
import type { On } from 'claude-code'

// The engine's side of what the relay calls. `row` is the one question the
// session asked, as mem lists it; `wait` is how a `mem questions --wait` ends.
function engine(on: On, wait: 'answered' | 'timed out' = 'answered') {
  const row = { short_id: 'qa', title: 'Which port', answered: false, answer: null as string | null }
  const runs: string[][] = []
  const prompts: string[] = []
  const statuses: (string | undefined)[] = []
  on('session.start', (_$, e) => ({ cwd: e.cwd }))
  on('turn.start', (_$, e) => ({ turnId: e.turnId }))
  on('turn.complete', (_$, e) => ({ text: e.answer }))
  on('session.id', () => ({ value: 'sessab' }))
  on('session.cwd', () => ({ value: '/work' }))
  on('ui.invalidate', () => ({ value: undefined }))
  on('ui.status', (_$, e) => {
    statuses.push(e.text)
    return { value: undefined }
  })
  on('prompt.submit', (_$, e) => {
    prompts.push(e.text)
    return { text: e.text }
  })
  on('process.run', (_$, e) => {
    runs.push([...e.argv, `timeout=${e.init?.timeoutMs}`])
    const asked = e.argv[1] === 'questions'
    return {
      value: {
        exitCode: 0,
        stdout: asked ? JSON.stringify({ questions: [row] }) : '{}',
        stderr: '',
        isStdoutTruncated: false,
        isStderrTruncated: false,
      },
    }
  })
  on('tool.call', { tool: 'Bash' }, (_$, e) => {
    if (e.command.startsWith('mem questions') && wait === 'timed out') {
      return { isError: true, result: 'mem: timed out', text: 'mem: timed out' }
    }
    return { result: 'ran' }
  })
  const answer = (text: string) => {
    row.answered = true
    row.answer = text
  }
  const relayRuns = () => runs.filter(r => r[1] === 'questions')
  return { runs, relayRuns, prompts, statuses, answer }
}

const done = { answer: 'done', durationMs: 1, isAborted: false, reason: 'answer' } as const
const LIST = ['mem', 'questions', '--asked-by', 'sessab', '--json', 'timeout=3000']

test('an answer arriving at 40 s is submitted once and the status cleared', async ($, on) => {
  mock.env(on, {})
  const clock = mock.clock(on)
  const { relayRuns, prompts, statuses, answer } = engine(on)

  await $.session.start({ cwd: '/work', surface: 'terminal', isInteractive: true })
  await $.turn.start({ text: 'ask', turnId: 't1' })
  await $.tool.call({ tool: 'Bash', command: 'mem ask "Which port"' })
  await $.turn.complete({ ...done, turnId: 't1' })
  expect(relayRuns()).toEqual([LIST])
  expect(statuses.at(-1)).toBe('waiting on hub #qa')

  await clock.advance(20000)
  expect(prompts).toEqual([])
  answer('8080')
  await clock.advance(20000)
  expect(prompts).toEqual(['Answer to #qa (Which port): 8080'])
  expect(statuses.at(-1)).toBeUndefined()

  await clock.advance(60000)
  await $.turn.complete({ ...done, turnId: 't2' })
  expect(prompts.length).toBe(1)
})

test('an answer a wait printed is not submitted again', async ($, on) => {
  mock.env(on, {})
  mock.clock(on)
  const { prompts, statuses, answer } = engine(on, 'answered')

  await $.session.start({ cwd: '/work', surface: 'terminal', isInteractive: true })
  await $.turn.start({ text: 'ask', turnId: 't1' })
  await $.tool.call({ tool: 'Bash', command: 'mem ask "Which port"' })
  answer('8080')
  await $.tool.call({ tool: 'Bash', command: 'mem questions --wait qa' })
  await $.turn.complete({ ...done, turnId: 't1' })

  expect(prompts).toEqual([])
  expect(statuses.at(-1)).toBeUndefined()
})

test('an answer after a wait that failed is submitted', async ($, on) => {
  mock.env(on, {})
  const clock = mock.clock(on)
  const { prompts, answer } = engine(on, 'timed out')

  await $.session.start({ cwd: '/work', surface: 'terminal', isInteractive: true })
  await $.turn.start({ text: 'ask', turnId: 't1' })
  await $.tool.call({ tool: 'Bash', command: 'mem ask "Which port"' })
  await $.tool.call({ tool: 'Bash', command: 'mem questions --wait qa' })
  await $.turn.complete({ ...done, turnId: 't1' })
  answer('8080')
  await clock.advance(20000)

  expect(prompts).toEqual(['Answer to #qa (Which port): 8080'])
})

test('a turn starting cancels the watch', async ($, on) => {
  mock.env(on, {})
  const clock = mock.clock(on)
  const { relayRuns } = engine(on)

  await $.session.start({ cwd: '/work', surface: 'terminal', isInteractive: true })
  await $.turn.start({ text: 'ask', turnId: 't1' })
  await $.tool.call({ tool: 'Bash', command: 'mem ask "Which port"' })
  await $.turn.complete({ ...done, turnId: 't1' })
  await $.turn.start({ text: 'more', turnId: 't2' })
  await clock.advance(60000)

  expect(relayRuns().length).toBe(1)
})

for (const [why, env, isInteractive, ask] of [
  ['before the session asks', {}, true, false],
  ["under the engine's mark", { WORKFLOW_ENGINE: '1' }, true, true],
  ['headless', {}, false, true],
] as const) {
  test(`no mem call ${why}`, async ($, on) => {
    mock.env(on, env)
    const clock = mock.clock(on)
    const { relayRuns, prompts, statuses } = engine(on)

    await $.session.start({ cwd: '/work', surface: 'terminal', isInteractive })
    await $.turn.start({ text: 'ask', turnId: 't1' })
    if (ask) await $.tool.call({ tool: 'Bash', command: 'mem ask "Which port"' })
    await $.turn.complete({ ...done, turnId: 't1' })
    await clock.advance(60000)

    expect(relayRuns()).toEqual([])
    expect(prompts).toEqual([])
    expect(statuses).toEqual([])
  })
}
