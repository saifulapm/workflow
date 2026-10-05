import { expect, mock, test } from 'claude-code/testing'
import type { On } from 'claude-code'

import { bandLine } from '../hooks/band'
import type { Brief } from '../hooks/band'

const running: Brief = {
  brief: '',
  project: 'workflow',
  roadmap_status: 'running',
  milestone: 'm7-mods',
  milestones_done: 7,
  milestones_total: 9,
  plan_slug: 'm7-mods',
  plan_ticked: 4,
  plan_total: 15,
  runner: 'macbook-m2',
  paused: false,
  questions_human: 2,
}

test('a running project shows its stage, milestone, tasks, questions and runner', () => {
  expect(bandLine(running)).toBe(
    'workflow · running · m7-mods (8/9) · tasks 4/15 · 2 questions · runner macbook-m2',
  )
})

test('a paused project says paused and leaves out a plan that is not the milestone', () => {
  const paused = { ...running, paused: true, plan_slug: 'm6-hub', questions_human: 1 }
  expect(bandLine(paused)).toBe('workflow · paused · m7-mods (8/9) · 1 question · runner macbook-m2')
})

test('outside a project mem knows the line is empty', () => {
  const none: Brief = {
    brief: 'handoff: elsewhere',
    project: null,
    roadmap_status: null,
    milestone: null,
    milestones_done: 0,
    milestones_total: 0,
    plan_slug: null,
    plan_ticked: 0,
    plan_total: 0,
    runner: null,
    paused: false,
    questions_human: 0,
  }
  expect(bandLine(none)).toBe('')
})

// The engine's side of what the band calls, counting each run of mem.
function engine(on: On) {
  const runs: string[][] = []
  on('session.start', (_$, e) => ({ cwd: e.cwd }))
  on('turn.start', (_$, e) => ({ turnId: e.turnId }))
  on('turn.complete', (_$, e) => ({ text: e.answer }))
  on('session.cwd', () => ({ value: '/work' }))
  on('ui.invalidate', () => ({ value: undefined }))
  on('process.run', (_$, e) => {
    runs.push([...e.argv])
    return {
      value: {
        exitCode: 0,
        stdout: JSON.stringify(running),
        stderr: '',
        isStdoutTruncated: false,
        isStderrTruncated: false,
      },
    }
  })
  return runs
}

const done = { answer: 'done', durationMs: 1, isAborted: false, reason: 'answer' } as const

test('mem runs at start, at each turn end and each idle minute', async ($, on) => {
  mock.env(on, {})
  const clock = mock.clock(on)
  const runs = engine(on)

  await $.session.start({ cwd: '/work', surface: 'terminal', isInteractive: true })
  expect(runs).toEqual([['mem', 'context', '--brief', '--json']])

  await $.turn.start({ text: 'hi', turnId: 't1' })
  await $.turn.complete({ ...done, turnId: 't1', agentId: 'a1' })
  expect(runs.length).toBe(1)
  await $.turn.complete({ ...done, turnId: 't1' })
  expect(runs.length).toBe(2)

  await clock.advance(60000)
  expect(runs.length).toBe(3)

  await $.turn.start({ text: 'again', turnId: 't2' })
  await clock.advance(60000)
  expect(runs.length).toBe(3)
  await $.turn.complete({ ...done, turnId: 't2' })
  expect(runs.length).toBe(4)
})

test('a headless session never runs mem', async ($, on) => {
  mock.env(on, {})
  const clock = mock.clock(on)
  const runs = engine(on)

  await $.session.start({ cwd: '/work', surface: 'terminal', isInteractive: false })
  await $.turn.start({ text: 'hi', turnId: 't1' })
  await $.turn.complete({ ...done, turnId: 't1' })
  await clock.advance(120000)
  expect(runs).toEqual([])
})
