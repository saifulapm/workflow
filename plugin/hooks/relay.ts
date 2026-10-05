// A question the session asked, as `mem questions --json` lists it.
export interface AskedRow {
  short_id: string
  title: string
  answered: boolean
  answer: string | null
}

export function asks(command: string): boolean {
  return /\bmem\s+ask(\s|$)/.test(command)
}

export function waits(command: string): boolean {
  return /\bmem\s+questions(\s|$)/.test(command) && /(^|\s)--wait(\s|=|$)/.test(command)
}

// Each answer reaches the session once: an answered row joins `delivered` the
// first time it is seen, and a quiet settle marks it without submitting it,
// for an answer the session already read from a `mem questions --wait`.
export function settleAnswers(
  rows: AskedRow[],
  delivered: Set<string>,
  quiet: boolean,
): { submit: AskedRow[]; watch: string[] } {
  const submit: AskedRow[] = []
  const watch: string[] = []
  for (const row of rows) {
    if (!row.answered) {
      watch.push(row.short_id)
    } else if (!delivered.has(row.short_id)) {
      delivered.add(row.short_id)
      if (!quiet) submit.push(row)
    }
  }
  return { submit, watch }
}
