#!/bin/bash
# Refreshes the cloud kit from this machine: Saiful's global rules (the
# dotfiles' ~/.claude/CLAUDE.md), the mem hooks of ~/.claude/settings.json,
# and the skills a cloud build needs beyond this repo's own. Run it after any
# of those change, then commit: CI ships cloud/ in the skills bundle, and
# `workflow cloud-setup` embeds it.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
skills_src=${SKILLS_SRC:-$HOME/.local/src/skills/skills}
kit_skills="frontend-design pnpm vite vitest"

cp "$(readlink -f "$HOME/.claude/CLAUDE.md")" "$here/rules.md"
jq '.hooks' "$HOME/.claude/settings.json" >"$here/settings-hooks.json"
rm -rf "$here/skills"
mkdir -p "$here/skills"
for name in $kit_skills; do
  cp -R "$skills_src/$name" "$here/skills/$name"
done
echo "cloud kit: CLAUDE.md, settings-hooks.json, skills: $kit_skills"
