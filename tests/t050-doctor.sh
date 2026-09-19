#!/usr/bin/env bash
# workflow doctor: it reports and never edits (AC3's hooksPath case, AC5b's
# settings keys).
source "$(dirname -- "$0")/lib.sh"
t_init

export WORKFLOW_SITES="$T_TMP/sites"
mkdir -p "$WORKFLOW_SITES"

settings="$HOME/.claude/settings.json"
mkdir -p "$HOME/.claude"

## ---------------------------------------------------------- a bare machine

run workflow doctor
is "$RC" 1 'an unwired machine has findings'
like "$OUT" 'no global core.hooksPath' 'and the first of them is the missing gate'
like "$OUT" 'settings' 'and the missing settings file'

## ------------------------------------------------------------- the wiring

git config --global core.hooksPath "$HOOKS"
workflow settings-merge "$settings" >/dev/null 2>&1
run workflow doctor
unlike "$OUT" 'no global core.hooksPath' 'with hooksPath set, the gate is no longer reported missing'
unlike "$OUT" 'WORKFLOW_AGENT is not' 'and the environment key is found'
unlike "$OUT" 'commitTrailers is not' 'and the commit trailers are off'
unlike "$OUT" 'sessionUrl is not' 'and the session url is off'

## -------------------------------------------------- two amx on PATH (#PPA68Q43)

# A run dispatches through whichever amx PATH answers first; a second one
# further down is where a "unexpected argument" refusal comes from.
mkdir -p "$T_TMP/amx-a" "$T_TMP/amx-b"
printf '#!/bin/sh\nexit 0\n' >"$T_TMP/amx-a/amx"; chmod +x "$T_TMP/amx-a/amx"
cp "$T_TMP/amx-a/amx" "$T_TMP/amx-b/amx"
wf=$(command -v workflow)
run env PATH="$T_TMP/amx-a:$T_TMP/amx-b:/usr/bin:/bin" "$wf" doctor
like "$OUT" "2 amx on PATH: $T_TMP/amx-a/amx answers, $T_TMP/amx-b/amx shadowed" 'doctor names both and which wins'
run env PATH="$T_TMP/amx-a:/usr/bin:/bin" "$wf" doctor
unlike "$OUT" 'amx on PATH' 'one amx is no finding'

## ---------------------------------------------- the settings merge (AC5b)

# This machine already sets attribution.commit and attribution.pr to the empty
# string. Empty means "say nothing", so it has to survive the merge.
cat >"$settings" <<'EOF'
{
  "model": "opus",
  "attribution": { "commit": "", "pr": "" },
  "env": { "SOMETHING_ELSE": "keep me" }
}
EOF
workflow settings-merge "$settings" >/dev/null 2>&1
is "$(jq -r '.attribution.commit' "$settings")" '' 'merge: the empty attribution.commit survives'
is "$(jq -r '.attribution | has("commit")' "$settings")" 'true' 'merge: and the key is still there'
is "$(jq -r '.attribution.pr' "$settings")" '' 'merge: the empty attribution.pr survives'
is "$(jq -r '.attribution.commitTrailers' "$settings")" 'false' 'merge: trailers are off'
is "$(jq -r '.attribution.sessionUrl' "$settings")" 'false' 'merge: session urls are off'
is "$(jq -r '.env.WORKFLOW_AGENT' "$settings")" '1' 'merge: the agent flag is set'
is "$(jq -r '.env.SOMETHING_ELSE' "$settings")" 'keep me' 'merge: other env keys are untouched'
is "$(jq -r '.model' "$settings")" 'opus' 'merge: unrelated settings are untouched'

before=$(cat "$settings")
run workflow settings-merge "$settings"
is "$(cat "$settings")" "$before" 'merge: running it again changes nothing'
like "$OUT" 'already' 'merge: and it says the file already said all of this'

# The merge writes through a temp file, and mktemp makes that 0600. The
# settings file's own permissions are not the merge's business.
chmod 640 "$settings"
printf '{"model":"opus"}\n' >"$settings"
chmod 640 "$settings"
workflow settings-merge "$settings" >/dev/null 2>&1
is "$(stat -c %a "$settings")" 640 'merge: the file keeps the mode it had'

rm -f "$settings"
workflow settings-merge "$settings" >/dev/null 2>&1
is "$(stat -c %a "$settings")" "$(printf '%o' "$((0666 & ~$(umask)))")" \
	'merge: a file it creates gets the mode the umask asks for'

# On this machine ~/.claude/settings.json is a chezmoi symlink into ~/.dotfiles.
# Writing over the link would leave the real file orphaned and unedited, the
# next `chezmoi apply` would revert the merge, and doctor would report healthy
# throughout because it reads through the link.
dotfiles="$T_TMP/dotfiles"
mkdir -p "$dotfiles"
printf '{"model":"opus","attribution":{"commit":"","pr":""}}\n' >"$dotfiles/settings.json"
chmod 640 "$dotfiles/settings.json"
rm -f "$settings"
ln -s "$dotfiles/settings.json" "$settings"
workflow settings-merge "$settings" >/dev/null 2>&1
is "$([ -L "$settings" ] && echo symlink || echo 'regular file')" symlink \
	'merge: a settings file that is a symlink is still a symlink afterwards'
is "$(readlink "$settings")" "$dotfiles/settings.json" 'merge: and it still points where it did'
is "$(jq -r '.env.WORKFLOW_AGENT' "$dotfiles/settings.json")" '1' \
	'merge: the edit landed in the file the link points at'
is "$(jq -r '.model' "$dotfiles/settings.json")" 'opus' \
	'merge: and the rest of that file survived'
is "$(stat -c %a "$dotfiles/settings.json")" 640 'merge: the target keeps the mode it had'
run workflow settings-merge "$settings"
like "$OUT" 'already' 'merge: and through the link it can tell it has nothing to do'

# Back to a plain wired settings file for the doctor runs below.
rm -f "$settings"
workflow settings-merge "$settings" >/dev/null 2>&1

## ---------------------------------------------------- the husky-shaped hole

repo="$WORKFLOW_SITES/laravel/shop"
mkdir -p "$repo"
git init -q "$repo"
git -C "$repo" config core.hooksPath .husky
run workflow doctor
is "$RC" 1 'a repo-local core.hooksPath is a finding'
like "$OUT" 'core.hooksPath=\.husky' 'and doctor names the repo and the path'
like "$OUT" 'beats the global one' 'and explains why it matters'

# doctor never edits: the repo keeps its own setting.
is "$(git -C "$repo" config --local --get core.hooksPath)" '.husky' 'doctor changed nothing'

## ------------------------------------------------------------- self-chain

repo2="$WORKFLOW_SITES/laravel/other"
mkdir -p "$repo2/.git/hooks"
git init -q "$repo2"
ln -sf "$HOOKS/pre-commit" "$repo2/.git/hooks/pre-commit"
run workflow doctor
like "$OUT" 'self-chain' 'the stub installed as a repo hook is reported'

rm -rf "$repo" "$repo2"

## ------------------------------------- an installed binary, an occupied slot

# An installed machine runs a copied binary with no checkout above the exe.
# Since the stubs and roles ride inside the binary, doctor needs no checkout:
# it compares the copies with its own text.
cp -L "$T_TMP/bin/workflow" "$T_TMP/installed-workflow"
chmod +x "$T_TMP/installed-workflow"

run "$T_TMP/installed-workflow" doctor
is "$RC" 1 'the installed binary has findings too'
unlike "$OUT" 'no workflow checkout found' 'no checkout is asked for: the binary carries the skills'
unlike "$OUT" 'healthy' 'an unverifiable machine is not called healthy'

## ------------------------------------------------------- the third door

# Still no WORKFLOW_HOME, and the installed binary still has no checkout
# above it, so ruling 8's third door is the checkout that owns the cwd: the
# parent of `git rev-parse --path-format=absolute --git-common-dir`,
# accepted only when that root itself holds hooks/pre-commit and skills/.

new_repo workflow
mkdir -p hooks skills/route
for name in pre-commit commit-msg pre-push; do
	write_exec "hooks/$name" <<'EOF'
#!/bin/sh
exit 0
EOF
done
printf -- '---\nname: route\ndescription: pick the lane\n---\n\nshort body\n' >skills/route/SKILL.md
git add hooks skills
git -c core.hooksPath=/dev/null commit -qm 'wiring'

wfhooks="$T_TMP/wfhooks"
mkdir -p "$wfhooks"
for name in pre-commit commit-msg pre-push; do
	ln -sf "$T_TMP/workflow/hooks/$name" "$wfhooks/$name"
done
git config --global core.hooksPath "$wfhooks"

run "$T_TMP/installed-workflow" doctor
unlike "$OUT" 'no workflow checkout found' \
	'a checkout holding hooks/pre-commit and skills/ answers the third door'

# A linked worktree of that checkout must still resolve to the main checkout,
# not to itself: every worktree the orchestrator makes is a full tree, so a
# marker check alone would not catch door three naming the worktree by
# mistake (review 2's regression).
git worktree add -q ../workflow-wt -b wt-branch
cd "$T_TMP/workflow-wt" || exit 1
run "$T_TMP/installed-workflow" doctor
unlike "$OUT" 'no workflow checkout found' \
	'from a linked worktree, door three still answers'
cd "$T_TMP" || exit 1

git config --global core.hooksPath "$HOOKS"

# The poshra repro from review 1 of doctor: an unrelated repo standing at
# this cwd, with neither marker, must still be refused.
new_repo other-project
run "$T_TMP/installed-workflow" doctor
is "$RC" 1 'an unrelated repo with neither marker is still a finding'
unlike "$OUT" 'no workflow checkout found' 'an unrelated repo at the cwd is no finding either'
cd "$T_TMP" || exit 1

## --------------------------------------------- the stubs, and the retiring

# A bare HOME has none of the three hook stubs at the fixed git hooks path.
# The eight skills are no longer written anywhere: they are served, by
# `workflow skill <name>` and `mem skill mem`.
export HOME="$T_TMP/embedded-home"
mkdir -p "$HOME"

run workflow doctor
is "$RC" 1 'a bare HOME has findings'
n=$(grep -c 'missing at' <<<"$OUT")
is "$n" 7 'seven entries are missing: the hook stubs and the four amx roles, and nothing else'
unlike "$OUT" 'skill route.*missing at' 'no skill is missing, because none is installed'
like "$OUT" 'hook pre-commit.*missing at .*\.config/git/hooks/pre-commit' \
	'a missing hook stub is named'
like "$OUT" 'role worker.*missing at .*\.config/amx/agents/worker\.md' \
	'a missing role is named where amx reads it'

run workflow doctor --fix
n=$(grep -c '^  .* wrote ' <<<"$OUT")
is "$n" 7 '--fix writes the three stubs and the four roles'
is "$(cat "$HOME/.config/git/hooks/pre-commit")" "$(cat "$WF_ROOT/hooks/pre-commit")" \
	'the stub holds the embedded text'
is "$(stat -c %a "$HOME/.config/git/hooks/pre-commit")" 755 'the stub is written executable'
# Where amx reads them: its config home is XDG's, which lib.sh pins under the
# first HOME, not the one this section moved to.
for role in worker reader fixer advisor; do
	is "$(cat "$XDG_CONFIG_HOME/amx/agents/$role.md")" "$(cat "$WF_ROOT/roles/$role.md")" \
		"the $role role holds the embedded text"
done
truthy "$([ ! -e "$HOME/.claude/skills" ] && echo 0 || echo 1)" \
	'--fix creates no claude skills directory'
truthy "$([ ! -e "$HOME/.agents/skills" ] && echo 0 || echo 1)" \
	'nor an agents one'

run workflow doctor
unlike "$OUT" 'missing at' 'a second doctor after --fix finds nothing missing'
unlike "$OUT" 'is installed' 'and a machine with no skills on disk says nothing about them'

# A machine upgraded from the version that installed them: the copies it wrote
# are still on disk, where every harness goes on discovering them.
for d in "$HOME/.claude/skills" "$HOME/.agents/skills"; do
	for s in route plan roadmap implement orchestrate review mem unslop; do
		mkdir -p "$d/$s"
		cp "$WF_ROOT/skills/$s/SKILL.md" "$d/$s/SKILL.md"
	done
done

run workflow doctor
is "$RC" 1 'the copies an older doctor installed are findings'
n=$(grep -c 'is installed; skills are served now' <<<"$OUT")
is "$n" 16 'sixteen of them: eight skills in two directories'
like "$OUT" 'skill route \(claude\).*is installed; skills are served now' \
	'each is named with its path'

run workflow doctor --fix
n=$(grep -c '^  .* retired ' <<<"$OUT")
is "$n" 16 '--fix retires all sixteen'
truthy "$([ ! -e "$HOME/.claude/skills/route" ] && echo 0 || echo 1)" \
	'the name directory is gone, not just the leaf'
truthy "$([ ! -e "$HOME/.agents/skills/mem" ] && echo 0 || echo 1)" \
	'mem_s copy goes too: workflow wrote it, so workflow takes it back out'

run workflow doctor
unlike "$OUT" 'is installed' 'and a retired machine says nothing about skills at all'
unlike "$OUT" 'retired ' 'with nothing left to retire'

## ------------------------------------------- a copy somebody edited by hand

# Text is what tells a copy this binary wrote from one somebody worked on.
# Deleting the second silently is data loss, so it is reported and left.
mkdir -p "$HOME/.claude/skills/route"
printf -- '---\nname: route\ndescription: mine\n---\n\nmy own notes\n' \
	>"$HOME/.claude/skills/route/SKILL.md"

run workflow doctor
like "$OUT" 'skill route \(claude\).*is not the copy this binary wrote' \
	'a hand-edited copy is named as such'

run workflow doctor --fix
like "$OUT" 'skill route \(claude\).*is not the copy this binary wrote' \
	'and --fix will not remove it either'
is "$(grep -c 'my own notes' "$HOME/.claude/skills/route/SKILL.md")" 1 \
	'the file is untouched'
rm -rf "$HOME/.claude/skills/route"

## --------------------------------------------- a symlinked name directory

# dotfiles linked the name directory, not the leaf: `ln -s <checkout>/skills/route
# ~/.claude/skills/route`. Removing that costs the link and never its target.
fake="$T_TMP/fake-checkout/skills/route"
mkdir -p "$fake"
printf 'content in the dev checkout\n' >"$fake/SKILL.md"
mkdir -p "$HOME/.claude/skills"
ln -s "$fake" "$HOME/.claude/skills/route"

run workflow doctor
like "$OUT" 'skill route \(claude\).*is installed' 'a symlinked name directory is found'

run workflow doctor --fix
like "$OUT" 'skill route \(claude\).*retired' 'and retired'
truthy "$([ ! -e "$HOME/.claude/skills/route" ] && echo 0 || echo 1)" 'the link is gone'
is "$(cat "$fake/SKILL.md")" 'content in the dev checkout' \
	'and the dev checkout it pointed at was never touched'

## --------------------------------------------------- a stub with no x bit

# Content alone is not enough: a copy tool that drops the mode leaves a stub
# git silently skips.
chmod 644 "$HOME/.config/git/hooks/pre-commit"
run workflow doctor
like "$OUT" 'hook pre-commit.*differs at' \
	'a correct-content stub missing its x bit is a finding, not healthy'

run workflow doctor --fix
is "$(stat -c %a "$HOME/.config/git/hooks/pre-commit")" 755 '--fix restores the x bit'

## ------------------------------------------------- a hooksPath that misses

# git runs whatever core.hooksPath names; a global hooksPath aimed elsewhere
# means the stubs doctor writes at the fixed path are never read.
foreign="$T_TMP/ghooks"
mkdir -p "$foreign"
printf '#!/bin/sh\nexit 0\n' >"$foreign/pre-push"
chmod +x "$foreign/pre-push"
git config --global core.hooksPath "$foreign"

run workflow doctor
like "$OUT" 'hooks path.*does not resolve to.*\.config/git/hooks' \
	'a hooksPath aimed elsewhere is a finding naming the fixed path'

git config --global core.hooksPath "$HOME/.config/git/hooks"
run workflow doctor
unlike "$OUT" 'does not resolve to' \
	'once hooksPath matches the fixed path, the finding is gone'
