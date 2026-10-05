// Any of these points git at another repository, so the staged diff the
// guard could read from the session's directory is not the one committed.
const ELSEWHERE = ['cd ', ' -C ', '--git-dir', '--work-tree']

// Whether a shell command runs `git commit` in the session's own directory.
export function commitsHere(command: string): boolean {
  if (!/\bgit\s+commit(\s|$)/.test(command)) return false
  return !ELSEWHERE.some(word => command.includes(word))
}

// One line, so it reads whole in the tool result the model sees.
export function refusal(finding: string): string {
  return `agent files do not belong in a product repo; put this in mem (mem save, mem wiki, mem decide) -- ${finding} -- WORKFLOW_MODS=off turns the workflow mods off`
}
