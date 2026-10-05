import { expect, test } from 'claude-code/testing'
import { commitsHere, refusal } from '../hooks/guard'
import { asks, settleAnswers, waits } from '../hooks/relay'
import type { AskedRow } from '../hooks/relay'

test('a git commit counts only when it runs in the session directory', () => {
  expect(commitsHere('git commit -m x')).toBe(true)
  expect(commitsHere('git add a && git commit')).toBe(true)
  expect(commitsHere('git -C ../o commit')).toBe(false)
  expect(commitsHere('cd o && git commit')).toBe(false)
  expect(commitsHere('git --git-dir=o/.git commit -m x')).toBe(false)
  expect(commitsHere('git --work-tree=o commit -m x')).toBe(false)
  expect(commitsHere('git commit-tree')).toBe(false)
  expect(commitsHere('git log')).toBe(false)
})

test('the refusal names the finding, mem and the switch on one line', () => {
  expect(refusal('CLAUDE.md is an agent file')).toBe(
    'agent files do not belong in a product repo; put this in mem (mem save, mem wiki, mem decide) -- CLAUDE.md is an agent file -- WORKFLOW_MODS=off turns the workflow mods off',
  )
})

test('asks reads a mem ask', () => {
  expect(asks('mem ask "which port?"')).toBe(true)
  expect(asks('mem questions --wait')).toBe(false)
  expect(asks('mem log "asked about the port"')).toBe(false)
})

test('waits reads a mem questions holding --wait', () => {
  expect(waits('mem questions --wait')).toBe(true)
  expect(waits('mem questions --json --wait')).toBe(true)
  expect(waits('mem questions')).toBe(false)
  expect(waits('mem ask --wait')).toBe(false)
})

const answered: AskedRow = { short_id: 'QWERTYUP', title: 'which port?', answered: true, answer: 'the default' }
const pending: AskedRow = { short_id: 'ZXCVBNMA', title: 'which host?', answered: false, answer: null }

test('an answered row is submitted once and a pending one watched', () => {
  const delivered = new Set<string>()
  expect(settleAnswers([answered, pending], delivered, false)).toEqual({
    submit: [answered],
    watch: ['ZXCVBNMA'],
  })
  expect([...delivered]).toEqual(['QWERTYUP'])

  expect(settleAnswers([answered, pending], delivered, false)).toEqual({
    submit: [],
    watch: ['ZXCVBNMA'],
  })
})

test('a quiet settle marks the answer delivered without submitting it', () => {
  const delivered = new Set<string>()
  expect(settleAnswers([answered], delivered, true)).toEqual({ submit: [], watch: [] })
  expect([...delivered]).toEqual(['QWERTYUP'])
  expect(settleAnswers([answered], delivered, false)).toEqual({ submit: [], watch: [] })
})
