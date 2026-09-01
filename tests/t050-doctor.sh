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
mkdir -p "$skills/route" "$skills/implement"
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
run workflow doctor
like "$OUT" 'skill route .*within budget' 'a small skill is within budget'
like "$OUT" 'skill implement .*within budget' 'implement gets the recorded 4800 byte exception'

{
	printf -- '---\nname: route\ndescription: pick the lane\n---\n\n'
	head -c 3400 /dev/zero | tr '\0' 'x'
	printf '\n'
} >"$skills/route/SKILL.md"
run workflow doctor
is "$RC" 1 'an oversized body is a finding'
like "$OUT" 'skill route.*body is 34[0-9][0-9] bytes' 'and the finding gives the size'

{
	printf -- '---\nname: route\n'
	printf 'description: '
	head -c 300 /dev/zero | tr '\0' 'x'
	printf -- '\n---\n\nshort body\n'
} >"$skills/route/SKILL.md"
run workflow doctor
like "$OUT" 'skill route.*frontmatter is' 'an oversized frontmatter is a finding too'

## ------------------------------------- an installed binary, an occupied slot

# An installed machine runs a copied binary: no checkout above the exe, so the
# hook-identity comparison has nothing to compare against. Doctor must say so
# instead of healthy, and must still catch a foreign hook squatting on a slot
# -- git-lfs writes its own pre-push into a global hooksPath
# (friction #13D9MGCP).
{
	printf -- '---\nname: route\ndescription: pick the lane\n---\n\n'
	printf 'short body\n'
} >"$skills/route/SKILL.md"
cp -L "$T_TMP/bin/workflow" "$T_TMP/installed-workflow"
chmod +x "$T_TMP/installed-workflow"

ghooks="$T_TMP/ghooks"
mkdir -p "$ghooks"
ln -sf "$HOOKS/pre-commit" "$ghooks/pre-commit"
ln -sf "$HOOKS/commit-msg" "$ghooks/commit-msg"
write_exec "$ghooks/pre-push" <<'EOF'
#!/bin/sh
command -v git-lfs >/dev/null 2>&1 || exit 0
git lfs pre-push "$@"
EOF
git config --global core.hooksPath "$ghooks"

run workflow doctor
is "$RC" 1 'a checkout binary flags the foreign file by path'
like "$OUT" 'hook pre-push.*does not resolve' 'and says where it points instead'

run "$T_TMP/installed-workflow" doctor
is "$RC" 1 'the installed binary has findings too'
like "$OUT" 'hook pre-push.*never invokes' 'the foreign pre-push is caught by content'
like "$OUT" 'git lfs update' 'and the remedy moves lfs into per-repo hooks'
unlike "$OUT" 'hook pre-commit' 'the real stubs pass the content check'
like "$OUT" 'WORKFLOW_HOME' 'unverifiable identity and budgets are said out loud'
unlike "$OUT" 'healthy' 'an unverifiable machine is not called healthy'

