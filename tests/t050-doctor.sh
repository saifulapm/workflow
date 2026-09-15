#!/usr/bin/env bash
# workflow doctor: it reports and never edits (AC3's hooksPath case, AC5b's
# settings keys, AC10's size budgets).
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

## ------------------------------------------------------- the size budgets

skills="$T_TMP/skills"
export WORKFLOW_SKILLS_DIR="$skills"
mkdir -p "$skills/route" "$skills/implement" "$skills/orchestrate"
{
	printf -- '---\nname: route\ndescription: pick the lane\n---\n\n'
	head -c 100 /dev/zero | tr '\0' 'x'
	printf '\n'
} >"$skills/route/SKILL.md"
{
	printf -- '---\nname: implement\ndescription: do the task\n---\n\n'
	head -c 4000 /dev/zero | tr '\0' 'x'
	printf '\n'
} >"$skills/implement/SKILL.md"
{
	printf -- '---\nname: orchestrate\ndescription: run the loop\n---\n\n'
	head -c 4000 /dev/zero | tr '\0' 'x'
	printf '\n'
} >"$skills/orchestrate/SKILL.md"
run workflow doctor
like "$OUT" 'skill route .*within budget' 'a small skill is within budget'
like "$OUT" 'skill implement .*within budget' 'implement gets the recorded 4800 byte exception'
like "$OUT" 'skill orchestrate .*within budget' 'orchestrate gets the recorded 4800 byte exception too'

{
	printf -- '---\nname: route\ndescription: pick the lane\n---\n\n'
	head -c 4000 /dev/zero | tr '\0' 'x'
	printf '\n'
} >"$skills/route/SKILL.md"
run workflow doctor
is "$RC" 1 'an oversized body is a finding'
like "$OUT" 'skill route.*body is 400[0-9] bytes' 'a route body of 4,000 bytes is over its 3,200 budget'

{
	printf -- '---\nname: route\n'
	printf 'description: '
	head -c 300 /dev/zero | tr '\0' 'x'
	printf -- '\n---\n\nshort body\n'
} >"$skills/route/SKILL.md"
run workflow doctor
like "$OUT" 'skill route.*frontmatter is' 'an oversized frontmatter is a finding too'

## ------------------------------------- an installed binary, an occupied slot

# An installed machine runs a copied binary with no checkout above the exe.
# Since the skills and stubs ride inside the binary, doctor needs no checkout:
# it measures its own skills and compares the copies with its own text.
{
	printf -- '---\nname: route\ndescription: pick the lane\n---\n\n'
	printf 'short body\n'
} >"$skills/route/SKILL.md"
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
# WORKFLOW_SKILLS_DIR is unset first so skill_sizes() has to fall back
# through checkout() to prove which root it got.
unset WORKFLOW_SKILLS_DIR

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
like "$OUT" 'skill route .*within budget' 'and the root it names is read for its own skills'

# A linked worktree of that checkout must still resolve to the main checkout,
# not to itself: every worktree the orchestrator makes is a full tree, so a
# marker check alone would not catch door three naming the worktree by
# mistake (review 2's regression).
git worktree add -q ../workflow-wt -b wt-branch
cd "$T_TMP/workflow-wt" || exit 1
run "$T_TMP/installed-workflow" doctor
unlike "$OUT" 'no workflow checkout found' \
	'from a linked worktree, door three still answers'
like "$OUT" 'skill route .*within budget' 'and the skills read are the main checkout'\''s'
cd "$T_TMP" || exit 1

git config --global core.hooksPath "$HOOKS"

# The poshra repro from review 1 of doctor: an unrelated repo standing at
# this cwd, with neither marker, must still be refused.
new_repo other-project
run "$T_TMP/installed-workflow" doctor
is "$RC" 1 'an unrelated repo with neither marker is still a finding'
unlike "$OUT" 'no workflow checkout found' 'an unrelated repo at the cwd is no finding either'
cd "$T_TMP" || exit 1

## ------------------------------------------- the embedded skills and stubs

# A bare HOME has none of the eight skills in either harness directory, and
# none of the three hook stubs at the fixed git hooks path: nineteen entries
# missing, none of them read through checkout() or WORKFLOW_SKILLS_DIR.
export HOME="$T_TMP/embedded-home"
mkdir -p "$HOME"

run workflow doctor
is "$RC" 1 'a bare HOME has findings'
n=$(grep -c 'missing at' <<<"$OUT")
is "$n" 19 'nineteen entries are missing: eight skills times two homes, plus three hooks'
like "$OUT" 'skill route \(claude\).*missing at .*\.claude/skills/route/SKILL\.md' \
	'a skill missing from the claude skills dir is named'
like "$OUT" 'skill route \(agents\).*missing at .*\.agents/skills/route/SKILL\.md' \
	'and the same skill missing from the agents skills dir'
like "$OUT" 'hook pre-commit.*missing at .*\.config/git/hooks/pre-commit' \
	'a missing hook stub is named too'

run workflow doctor --fix
n=$(grep -c '^  .* wrote ' <<<"$OUT")
is "$n" 19 '--fix writes all nineteen'
is "$(cat "$HOME/.claude/skills/route/SKILL.md")" "$(cat "$WF_ROOT/skills/route/SKILL.md")" \
	'the claude copy matches the embedded text'
is "$(cat "$HOME/.agents/skills/route/SKILL.md")" "$(cat "$WF_ROOT/skills/route/SKILL.md")" \
	'and so does the agents copy'
is "$(cat "$HOME/.config/git/hooks/pre-commit")" "$(cat "$WF_ROOT/hooks/pre-commit")" \
	'and the hook stub'
is "$(stat -c %a "$HOME/.config/git/hooks/pre-commit")" 755 'the stub is written executable'

run workflow doctor
unlike "$OUT" 'missing at' 'a second doctor after --fix finds nothing missing'
unlike "$OUT" 'differs at' 'nor anything differing'
unlike "$OUT" 'symlink at' 'nor a symlink'

printf 'edited\n' >>"$HOME/.claude/skills/route/SKILL.md"
run workflow doctor
like "$OUT" 'skill route \(claude\).*differs at' 'an edited copy is reported as differing'
unlike "$OUT" 'skill route \(agents\).*differs at' 'the untouched copy is not'

rm "$HOME/.agents/skills/mem/SKILL.md"
ln -s "$WF_ROOT/skills/mem/SKILL.md" "$HOME/.agents/skills/mem/SKILL.md"
run workflow doctor
like "$OUT" 'skill mem \(agents\).*symlink at' \
	'a symlinked skill is reported as a symlink, not as healthy'

run workflow doctor --fix
like "$OUT" 'skill mem \(agents\).*wrote' 'fixing the symlink is one of the lines --fix prints'
is "$([ -L "$HOME/.agents/skills/mem/SKILL.md" ] && echo symlink || echo 'regular file')" \
	'regular file' 'the symlink is replaced by a plain copy'
is "$(cat "$HOME/.agents/skills/mem/SKILL.md")" "$(cat "$WF_ROOT/skills/mem/SKILL.md")" \
	'holding the embedded text'

## ---------------------------------------------- a symlinked name directory

# dotfiles link the name directory, not the leaf: `ln -s <checkout>/skills/route
# ~/.claude/skills/route`. The leaf SKILL.md this reaches is then a regular
# file read through the link, so is_symlink() on the leaf alone misses it.
fake="$T_TMP/fake-checkout/skills/route"
mkdir -p "$fake"
printf 'stale content from the dev checkout\n' >"$fake/SKILL.md"
rm -rf "$HOME/.claude/skills/route"
ln -s "$fake" "$HOME/.claude/skills/route"

run workflow doctor
like "$OUT" 'skill route \(claude\).*symlink at' \
	'a symlinked name directory is reported as a symlink, not as differs'

run workflow doctor --fix
like "$OUT" 'skill route \(claude\).*wrote' 'fixing it is one of the lines --fix prints'
is "$([ -L "$HOME/.claude/skills/route" ] && echo symlink || echo 'regular dir')" \
	'regular dir' 'the link is replaced by a real directory'
is "$(cat "$HOME/.claude/skills/route/SKILL.md")" "$(cat "$WF_ROOT/skills/route/SKILL.md")" \
	'holding the embedded text'
is "$(cat "$fake/SKILL.md")" 'stale content from the dev checkout' \
	'and the dev checkout the link pointed at was never touched'

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
