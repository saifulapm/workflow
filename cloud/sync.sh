#!/bin/bash
# Refreshes the cloud kit from this machine: Saiful's global rules (the
# dotfiles' ~/.claude/CLAUDE.md), the global git ignore list, the mem hooks of ~/.claude/settings.json
# with the cloud's own Stop hook, and the skills a cloud build needs beyond this repo's own. Run it after any
# of those change, then commit: CI ships cloud/ in the skills bundle, and
# `workflow cloud-setup` embeds it. settings-keys.json (the advisor, the locale, Composer as root,
# and the local settings that fit a thread) and settings-deny.json (the local Read rules and the
# claude.ai connectors a thread must not load, which disableClaudeAiConnectors leaves; merged into
# the image's own) are kept by hand: the cloud's advisor may differ from this machine's.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
skills_src=${SKILLS_SRC:-$HOME/.local/src/skills/skills}
kit_skills="frontend-design pnpm vite vitest"

cp "$(readlink -f "$HOME/.claude/CLAUDE.md")" "$here/rules.md"
# The global ignore list (core.excludesFile), so agent files stay untracked
# in a thread's checkout too.
cp "$(readlink -f "$HOME/.config/git/ignore")" "$here/gitignore"
# The Stop hook is the cloud's own: a cloud session's VM goes offline when
# its turn ends, so it waits in the turn for the answer to its question.
# So is the last SessionStart hook: a cloud session's start and each resume
# write `hooks ran: <session> in <project of the cwd, or none>` to
# workflow's log, so mem shows whether a thread's hooks ran. It prints
# nothing, waits on mem 5 s at most, and never fails the start. The
# project is the cwd's: a session's MEM_PROJECT names its own.
# shellcheck disable=SC2016
ran='[ -z "$CLAUDE_CODE_REMOTE_SESSION_ID" ] || { p=$(env -u MEM_PROJECT timeout 5 mem project current 2>/dev/null | sed -n "s/^name  *//p"); timeout 5 mem log --project workflow "hooks ran: $CLAUDE_CODE_REMOTE_SESSION_ID in ${p:-none}" >/dev/null 2>&1; }; true'
jq --arg ran "$ran" '.hooks + {SessionStart: (.hooks.SessionStart + [{hooks: [{type: "command", command: $ran}]}]),
  Stop: [{hooks: [{type: "command",
  command: "mem stop-hook --session-id \"$CLAUDE_CODE_SESSION_ID\" || true"}]}]}' \
  "$HOME/.claude/settings.json" >"$here/settings-hooks.json"
rm -rf "$here/skills"
mkdir -p "$here/skills"
for name in $kit_skills; do
  cp -R "$skills_src/$name" "$here/skills/$name"
done
echo "cloud kit: CLAUDE.md, gitignore, settings-hooks.json, skills: $kit_skills"
