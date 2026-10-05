import { expect, mock, test } from 'claude-code/testing'
import type { On } from 'claude-code'

const FINDING = 'CLAUDE.md:0: hard agent file: this belongs in mem'

// The engine's side of what the guard calls: each run of the binary is
// counted and answered with `exit`, or rejected when `exit` is a string.
function engine(on: On, exit: number | string) {
  const runs: string[][] = []
  const toasts: string[] = []
  const written: string[] = []
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
        stderr: exit === 2 ? "error: unexpected argument '--would-create' found\n" : '',
        isStdoutTruncated: false,
        isStderrTruncated: false,
      },
    }
  })
  on('tool.call', { tool: ['Write', 'Edit', 'NotebookEdit'] }, (_$, e) => {
    written.push(e.tool === 'NotebookEdit' ? e.notebook_path : e.file_path)
    return { result: 'written' }
  })
  return { runs, toasts, written }
}

test('a new instruction file is refused, naming the switch, with one toast', async ($, on) => {
  mock.env(on, {})
  const { runs, toasts, written } = engine(on, 1)

  const call = await $.tool.call({ tool: 'Write', file_path: '/work/CLAUDE.md', content: 'x' })

  expect(runs).toEqual([
    ['workflow', 'hygiene', '--would-create', '/work/CLAUDE.md', 'cwd=/work', 'timeout=3000'],
  ])
  expect(call.deny).toBe(
    `agent files do not belong in a product repo; put this in mem (mem save, mem wiki, mem decide) -- ${FINDING} -- WORKFLOW_MODS=off turns the workflow mods off`,
  )
  expect(toasts.length).toBe(1)
  expect(toasts[0]).toContain('/work/CLAUDE.md')
  expect(written).toEqual([])
})

for (const [why, exit] of [
  ['exit 0', 0],
  ['exit 2 from a binary that does not know the flag', 2],
  ['a binary that cannot start', 'spawn workflow ENOENT'],
] as const) {
  test(`${why} lets the write through`, async ($, on) => {
    mock.env(on, {})
    const { runs, toasts, written } = engine(on, exit)

    const call = await $.tool.call({ tool: 'Edit', file_path: '/work/src/a.rs', old_string: 'a', new_string: 'b' })

    expect(runs.length).toBe(1)
    expect(call.deny).toBeUndefined()
    expect(written).toEqual(['/work/src/a.rs'])
    expect(toasts).toEqual([])
  })
}

test('a notebook edit is asked about by its notebook path', async ($, on) => {
  mock.env(on, {})
  const { runs, written } = engine(on, 0)

  await $.tool.call({ tool: 'NotebookEdit', notebook_path: '/work/a.ipynb', new_source: 'x' })

  expect(runs.map(r => r[3])).toEqual(['/work/a.ipynb'])
  expect(written).toEqual(['/work/a.ipynb'])
})

test('with WORKFLOW_MODS=off the binary is never run', async ($, on) => {
  mock.env(on, { WORKFLOW_MODS: 'off' })
  const { runs, written } = engine(on, 1)

  await $.tool.call({ tool: 'Write', file_path: '/work/CLAUDE.md', content: 'x' })

  expect(runs).toEqual([])
  expect(written).toEqual(['/work/CLAUDE.md'])
})
