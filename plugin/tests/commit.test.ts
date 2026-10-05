import { expect, mock, test } from 'claude-code/testing'
import type { On } from 'claude-code'

// An ignore-list finding, since a hard-tier phrase in this file would trip
// the pre-commit hook on the file itself.
const FINDING = 'CLAUDE.md:0: hard ignored path: CLAUDE.md'

// The engine's side of what the commit check calls: each run of the binary
// is counted and answered with `exit`, or rejected when `exit` is a string.
function engine(on: On, exit: number | string) {
  const runs: string[][] = []
  const toasts: string[] = []
  const ran: string[] = []
  on('session.cwd', () => ({ value: '/work' }))
  on('ui.toast', (_$, e) => {
    toasts.push(JSON.stringify(e))
    return { value: undefined }
  })
  on('process.run', (_$, e) => {
    runs.push([...e.argv, `cwd=${e.init?.cwd}`, `timeout=${e.init?.timeoutMs}`])
    if (typeof exit === 'string') throw new Error(exit)
    return {
      value: {
        exitCode: exit,
        stdout: exit === 1 ? `${FINDING}\n` : '',
        stderr: exit === 2 ? "error: unexpected argument '--known' found\n" : '',
        isStdoutTruncated: false,
        isStderrTruncated: false,
      },
    }
  })
  on('tool.call', { tool: 'Bash' }, (_$, e) => {
    ran.push(e.command)
    return { result: 'ran' }
  })
  return { runs, toasts, ran }
}

test('a commit the hard tier refuses is denied with the finding, with one toast', async ($, on) => {
  mock.env(on, {})
  const { runs, toasts, ran } = engine(on, 1)

  const call = await $.tool.call({ tool: 'Bash', command: 'git commit -m x' })

  expect(runs).toEqual([['workflow', 'hygiene', '--staged', '--known', 'cwd=/work', 'timeout=5000']])
  expect(call.deny).toBe(
    `agent files do not belong in a product repo; put this in mem (mem save, mem wiki, mem decide) -- ${FINDING} -- WORKFLOW_MODS=off turns the workflow mods off`,
  )
  expect(toasts.length).toBe(1)
  expect(ran).toEqual([])
})

for (const [why, exit] of [
  ['a clean diff', 0],
  ['exit 2 from a binary that does not know the flag', 2],
  ['a binary that cannot start', 'spawn workflow ENOENT'],
] as const) {
  test(`${why} lets the commit run`, async ($, on) => {
    mock.env(on, {})
    const { runs, toasts, ran } = engine(on, exit)

    const call = await $.tool.call({ tool: 'Bash', command: 'git commit -m x' })

    expect(runs.length).toBe(1)
    expect(call.deny).toBeUndefined()
    expect(ran).toEqual(['git commit -m x'])
    expect(toasts).toEqual([])
  })
}

for (const command of ['git -C ../o commit', 'ls']) {
  test(`${command} runs no process`, async ($, on) => {
    mock.env(on, {})
    const { runs, ran } = engine(on, 1)

    await $.tool.call({ tool: 'Bash', command })

    expect(runs).toEqual([])
    expect(ran).toEqual([command])
  })
}
