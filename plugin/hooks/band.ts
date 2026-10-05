// What `mem context --brief --json` prints: the brief and where this
// checkout's project stands, `project` null outside one mem knows.
export type Brief = {
  brief: string
  project: string | null
  roadmap_status: string | null
  milestone: string | null
  milestones_done: number
  milestones_total: number
  plan_slug: string | null
  plan_ticked: number
  plan_total: number
  runner: string | null
  paused: boolean
  questions_human: number
}

// The band's line: each part left out when it has nothing to say, and the
// whole line empty outside a project, where a mem from before the project
// keys leaves `project` undefined too.
export function bandLine(b: Brief): string {
  if (!b.project) return ''
  const parts = [b.project]
  if (b.paused) parts.push('paused')
  else if (b.roadmap_status) parts.push(b.roadmap_status)
  if (b.milestone) parts.push(`${b.milestone} (${b.milestones_done + 1}/${b.milestones_total})`)
  if (b.milestone && b.plan_slug === b.milestone) parts.push(`tasks ${b.plan_ticked}/${b.plan_total}`)
  if (b.questions_human === 1) parts.push('1 question')
  else if (b.questions_human > 1) parts.push(`${b.questions_human} questions`)
  if (b.runner) parts.push(`runner ${b.runner}`)
  return parts.join(' · ')
}
