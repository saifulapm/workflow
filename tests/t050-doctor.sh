#!/usr/bin/env bash
# workflow doctor: it reports and never edits (AC3's hooksPath case, AC5b's
# settings keys).
source "$(dirname -- "$0")/lib.sh"
t_init

export WORKFLOW_SITES="$T_TMP/sites"
mkdir -p "$WORKFLOW_SITES"

settings="$HOME/.claude/settings.json"
mkdir -p "$HOME/.claude"

# systemctl answers is-enabled from a file, so the unit's state is the test's
# and not the machine's.
unit_state="$T_TMP/unit-state"
echo enabled >"$unit_state"
write_exec "$T_TMP/bin/systemctl" <<EOF
#!/bin/sh
[ "\$*" = '--user is-enabled workflow.service' ] || exit 1
cat '$unit_state'
EOF

## ---------------------------------------------------------- a bare machine

run workflow doctor
is "$RC" 1 'an unwired machine has findings'
like "$OUT" 'no global core.hooksPath' 'and the first of them is the missing gate'
like "$OUT" 'settings' 'and the missing settings file'

## ------------------------------------------------------------- the wiring

git config --global core.hooksPath "$HOOKS"
cat >"$settings" <<'EOF'
{
  "env": { "WORKFLOW_AGENT": "1" },
  "attribution": { "commitTrailers": false, "sessionUrl": false }
}
EOF
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

## ------------------------------------------------ the stubs, roles and skills

# A bare HOME has none of the hook stubs, roles, skills or plugin files. Every
# skill the binary carries, and mem's, is installed as a file in both skills
# directories, so each harness finds it where it looks; the plugin goes under
# Claude Code's alone.
export HOME="$T_TMP/embedded-home"
mkdir -p "$HOME"

skills=($(workflow skill | sed 's/ — .*//') mem)
roles=(worker lead dogfood review research plan plan-refresh grill)
truthy "$([ "${#skills[@]}" -gt 1 ] && echo 0 || echo 1)" 'workflow skill names the skills it carries'
is "${#roles[@]}" "$(ls "$WF_ROOT"/roles/*.md | wc -l)" 'the role list is every file under roles/'
plugin=(.claude-plugin/plugin.json hooks/hooks.json hooks/register.ts hooks/band.ts hooks/guard.ts hooks/relay.ts)
installed="$HOME/.claude/skills/workflow"
copies=$((4 + ${#roles[@]} + 2 * ${#skills[@]} + ${#plugin[@]}))

run workflow doctor
is "$RC" 1 'a bare HOME has findings'
n=$(grep -c 'missing at' <<<"$OUT")
is "$n" "$copies" 'the hook stubs, every role, every skill in both dirs and the plugin are missing'
for s in "${skills[@]}"; do
	like "$OUT" "skill $s \\(claude\\).*missing at $HOME/\\.claude/skills/$s/SKILL\\.md" \
		"a missing $s skill is named in the claude dir"
	like "$OUT" "skill $s \\(agents\\).*missing at $HOME/\\.agents/skills/$s/SKILL\\.md" \
		"and in the agents dir"
done
for f in "${plugin[@]}"; do
	like "$OUT" "plugin $f.*missing at $installed/$f" "a missing plugin file $f is named"
done
like "$OUT" 'hook pre-commit.*missing at .*\.config/git/hooks/pre-commit' \
	'a missing hook stub is named'
like "$OUT" 'role worker.*missing at .*\.config/amx/agents/worker\.md' \
	'a missing role is named where amx reads it'
like "$OUT" 'role review.*missing at .*\.config/amx/agents/review\.md' \
	'a missing review role is named too'
unit="$XDG_CONFIG_HOME/systemd/user/workflow.service"
like "$OUT" "unit workflow\\.service.*missing at $unit" 'a missing service unit is named'

run workflow doctor --fix
n=$(grep -c '^  .* wrote ' <<<"$OUT")
is "$n" "$copies" '--fix writes the stubs, the roles, the skills and the plugin'
is "$(cat "$HOME/.config/git/hooks/pre-commit")" "$(cat "$WF_ROOT/hooks/pre-commit")" \
	'the stub holds the embedded text'
is "$(stat -c %a "$HOME/.config/git/hooks/pre-commit")" 755 'the stub is written executable'
# With no dotfiles unit directory the unit is a plain file in the config home.
truthy "$([ -f "$unit" ] && [ ! -L "$unit" ] && echo 0 || echo 1)" \
	'with no dotfiles the unit is written straight to the config home'
for line in 'ExecStart=%h/.local/bin/workflow serve' 'ConditionPathExists=%h/.local/bin/workflow' \
	'Environment=PATH=%h/.local/share/mise/shims:%h/.local/share/pnpm:%h/.local/bin:%h/.cargo/bin:/usr/local/bin:/usr/bin' 'Restart=always' \
	'RestartSec=5' 'TimeoutStopSec=60' 'WantedBy=default.target'; do
	truthy "$(grep -qxF -- "$line" "$unit" && echo 0 || echo 1)" "the unit holds $line"
done
unlike "$OUT" 'commit the dotfiles' 'and nothing under the dotfiles asks for a commit'
# Where amx reads them: its config home is XDG's, which lib.sh pins under the
# first HOME, not the one this section moved to.
for role in "${roles[@]}"; do
	is "$(cat "$XDG_CONFIG_HOME/amx/agents/$role.md")" "$(cat "$WF_ROOT/roles/$role.md")" \
		"the $role role holds the embedded text"
done
for s in "${skills[@]}"; do
	for d in "$HOME/.claude/skills" "$HOME/.agents/skills"; do
		is "$(cat "$d/$s/SKILL.md")" "$(cat "$WF_ROOT/skills/$s/SKILL.md")" \
			"the $s skill in $d holds the embedded text"
	done
done

for f in "${plugin[@]}"; do
	is "$(cat "$installed/$f")" "$(cat "$WF_ROOT/plugin/$f")" "the plugin file $f holds the embedded text"
done

run workflow doctor
unlike "$OUT" 'missing at' 'a second doctor after --fix finds nothing missing'
unlike "$OUT" 'plugin ' 'and says nothing about the plugin'
unlike "$OUT" 'skill [a-z-]+ \(' 'and says nothing about the skills'
unlike "$OUT" 'retired|served' 'and no line says skills are served or retired'

## ---------------------------------------------- a copy that drifted

# The binary is the only source: a copy that differs is reported, and --fix
# writes the binary's text over it.
printf -- '---\nname: route\ndescription: mine\n---\n\nmy own notes\n' \
	>"$HOME/.claude/skills/route/SKILL.md"

run workflow doctor
is "$RC" 1 'a drifted skill copy is a finding'
like "$OUT" "skill route \\(claude\\).*differs at $HOME/\\.claude/skills/route/SKILL\\.md" \
	'named as differing, with its path'

run workflow doctor --fix
like "$OUT" 'skill route \(claude\).*wrote ' '--fix rewrites it'
is "$(cat "$HOME/.claude/skills/route/SKILL.md")" "$(cat "$WF_ROOT/skills/route/SKILL.md")" \
	'and the copy holds the embedded text again'

# The same goes for the plugin: a file gone and a file edited are each named.
rm "$installed/hooks/band.ts"
printf '// my own hooks\n' >>"$installed/hooks/register.ts"
run workflow doctor
is "$RC" 1 'a drifted plugin is a finding'
like "$OUT" "plugin hooks/band\\.ts.*missing at $installed/hooks/band\\.ts" 'the missing file is named'
like "$OUT" "plugin hooks/register\\.ts.*differs at $installed/hooks/register\\.ts" \
	'the edited file is named as differing'

run workflow doctor --fix
for f in hooks/band.ts hooks/register.ts; do
	is "$(cat "$installed/$f")" "$(cat "$WF_ROOT/plugin/$f")" "--fix restores $f"
done

## --------------------------------------------- a symlinked name directory

# dotfiles linked the name directory, not the leaf: `ln -s <checkout>/skills/route
# ~/.claude/skills/route`. --fix replaces the link with a plain copy and never
# touches its target.
fake="$T_TMP/fake-checkout/skills/route"
mkdir -p "$fake"
printf 'content in the dev checkout\n' >"$fake/SKILL.md"
rm -rf "$HOME/.claude/skills/route"
ln -s "$fake" "$HOME/.claude/skills/route"

run workflow doctor
like "$OUT" 'skill route \(claude\).*symlink at' 'a symlinked name directory is found'

run workflow doctor --fix
like "$OUT" 'skill route \(claude\).*wrote ' 'and replaced'
truthy "$([ ! -L "$HOME/.claude/skills/route" ] && echo 0 || echo 1)" 'the link is gone'
is "$(cat "$HOME/.claude/skills/route/SKILL.md")" "$(cat "$WF_ROOT/skills/route/SKILL.md")" \
	'a plain copy stands in its place'
is "$(cat "$fake/SKILL.md")" 'content in the dev checkout' \
	'and the dev checkout it pointed at was never touched'

# A linked `workflow` directory sits two levels above the plugin's hook
# files; --fix must replace the link, not write through it.
fakeplug="$T_TMP/fake-checkout/plugin"
mkdir -p "$fakeplug/hooks"
printf 'register in the dev checkout\n' >"$fakeplug/hooks/register.ts"
rm -rf "$installed"
ln -s "$fakeplug" "$installed"

run workflow doctor
like "$OUT" "plugin hooks/register\\.ts.*symlink at $installed/hooks/register\\.ts" \
	'a symlinked workflow directory is found from a file below it'

run workflow doctor --fix
like "$OUT" 'plugin hooks/register\.ts.*wrote ' 'and replaced'
truthy "$([ ! -L "$installed" ] && echo 0 || echo 1)" 'the link is gone'
is "$(cat "$installed/hooks/register.ts")" "$(cat "$WF_ROOT/plugin/hooks/register.ts")" \
	'a plain copy stands in its place'
is "$(cat "$fakeplug/hooks/register.ts")" 'register in the dev checkout' \
	'and the directory it pointed at was never touched'
is "$(find "$fakeplug" -type f | wc -l)" 1 'nor written into'

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

## ------------------------------------- the ignore list and the advisor

# Every repo on the machine ignores the agent files through one global list
# the dotfiles install, and the advisor runs on keys in the settings file.
# doctor checks both and writes neither: --fix prints the edit to make.
wanted=(.claude/ CLAUDE.md AGENTS.md .scratch/ .amx/)
ignore="$HOME/.config/git/ignore"
printf '%s\n' "${wanted[@]}" >"$ignore"
git config --global core.excludesFile "$ignore"

settings="$HOME/.claude/settings.json"
mkdir -p "$HOME/.claude"
cat >"$settings" <<'EOF'
{
  "advisorModel": "fable",
  "env": { "WORKFLOW_AGENT": "1", "CLAUDE_CODE_SUBAGENT_MODEL": "sonnet" },
  "attribution": { "commitTrailers": false, "sessionUrl": false }
}
EOF
good=$(cat "$settings")

# One amx or none on PATH, whatever the machine running the suite has.
doctor() { env PATH="$T_TMP/bin:/usr/bin:/bin" "$wf" doctor "$@"; }

run doctor
is "$RC" 0 'a fully wired machine has no findings'
like "$OUT" 'healthy: nothing to fix' 'and says so'

git config --global --unset core.excludesFile
run doctor
is "$RC" 1 'no global excludesFile is a finding'
like "$OUT" 'ignore list.*no global core\.excludesFile' 'and doctor says which key'
git config --global core.excludesFile "$ignore"

for line in "${wanted[@]}"; do
	printf '%s\n' "${wanted[@]}" | grep -vxF -- "$line" >"$ignore"
	run doctor
	is "$RC" 1 "an ignore list without $line is a finding"
	like "$OUT" "ignore list.*$ignore lacks $line" 'naming the file and the line'
done
printf '%s\n' "${wanted[@]}" >"$ignore"

set_settings() { jq "$@" <<<"$good" >"$settings"; }

set_settings 'del(.advisorModel)'
run doctor
is "$RC" 1 'settings without advisorModel are a finding'
like "$OUT" 'settings advisor.*advisorModel is not set' 'and doctor names the key'

set_settings '.env.CLAUDE_CODE_SUBAGENT_MODEL = "opus"'
run doctor
is "$RC" 1 'a subagent model other than sonnet is a finding'
like "$OUT" 'settings env.*env\.CLAUDE_CODE_SUBAGENT_MODEL is not "sonnet"' 'and doctor names it'

set_settings 'del(.env.CLAUDE_CODE_SUBAGENT_MODEL)'
run doctor
is "$RC" 1 'no subagent model is a finding too'
like "$OUT" 'env\.CLAUDE_CODE_SUBAGENT_MODEL is not "sonnet"' 'with the same line'

for key in CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC DISABLE_TELEMETRY DO_NOT_TRACK; do
	set_settings --arg k "$key" '.env[$k] = "1"'
	run doctor
	is "$RC" 1 "env.$key is a finding"
	like "$OUT" "settings env.*env\\.$key is set" 'and doctor names it'
done

# All of it wrong at once, and --fix: the edits are printed, nothing written.
printf '.claude/\n' >"$ignore"
set_settings 'del(.advisorModel) | .env.CLAUDE_CODE_SUBAGENT_MODEL = "opus" | .env.DO_NOT_TRACK = "1"'
before_settings=$(cat "$settings")
run doctor --fix
is "$RC" 1 '--fix leaves these findings standing'
like "$OUT" "dotfiles edit.*add CLAUDE\\.md to $ignore" '--fix prints the ignore line to add'
like "$OUT" "dotfiles edit.*set advisorModel in $settings" 'and the advisor key to set'
like "$OUT" "dotfiles edit.*set env\\.CLAUDE_CODE_SUBAGENT_MODEL to \"sonnet\" in $settings" \
	'and the subagent model to set'
like "$OUT" "dotfiles edit.*remove env\\.DO_NOT_TRACK from $settings" 'and the key to remove'
is "$(cat "$settings")" "$before_settings" '--fix writes nothing to settings'
is "$(cat "$ignore")" '.claude/' 'nor to the ignore list'

# A settings file linked into the dotfiles is edited at the link's target.
dotfiles="$T_TMP/dotfiles"
mkdir -p "$dotfiles"
mv "$settings" "$dotfiles/linked.json"
ln -s "$dotfiles/linked.json" "$settings"
run doctor --fix
like "$OUT" "dotfiles edit.*set advisorModel in $(realpath "$dotfiles/linked.json")" \
	'a linked settings file is named by its target'
rm -f "$settings"
mv "$dotfiles/linked.json" "$settings"

git config --global --unset core.excludesFile
run doctor --fix
like "$OUT" 'dotfiles edit.*set core\.excludesFile' '--fix says to set the key when it is unset'
is "$(git config --global --get core.excludesFile)" '' 'and does not set it'

## ------------------------------------------------------------ the service unit

# Back to a fully wired machine, so the unit is all doctor has to say.
git config --global core.excludesFile "$ignore"
printf '%s\n' "${wanted[@]}" >"$ignore"
printf '%s\n' "$good" >"$settings"

# A drifted unit in the config home is named and rewritten.
printf '[Service]\nExecStart=/bin/false\n' >"$unit"
run doctor
is "$RC" 1 'a drifted unit is a finding'
like "$OUT" "unit workflow\\.service.*differs at $unit" 'named as differing, with its path'

# With the dotfiles' unit directory present, the unit goes there and the config
# home links to it, the way chezmoi lays out hub.service.
units="$HOME/.dotfiles/home/dot_config/systemd/user"
mkdir -p "$units"
run doctor --fix
like "$OUT" "unit workflow\\.service.*wrote $units/workflow\\.service" '--fix writes the unit under the dotfiles'
like "$OUT" 'commit the dotfiles' 'and asks for the dotfiles to be committed'
truthy "$([ -L "$unit" ] && echo 0 || echo 1)" 'the config home holds a link'
is "$(readlink "$unit")" "$units/workflow.service" 'to the dotfiles copy'
truthy "$(grep -qxF 'ExecStart=%h/.local/bin/workflow serve' "$units/workflow.service" && echo 0 || echo 1)" \
	'and the dotfiles copy holds the unit'

run doctor
is "$RC" 0 'a linked unit is healthy'
unlike "$OUT" 'unit workflow' 'and doctor says nothing about it'

printf '[Service]\nExecStart=/bin/false\n' >"$units/workflow.service"
run doctor
is "$RC" 1 'a drifted dotfiles unit is a finding'
like "$OUT" "unit workflow\\.service.*differs at $units/workflow\\.service" 'named at the dotfiles path'
run doctor --fix
run doctor
is "$RC" 0 '--fix rewrites the dotfiles copy'

# Enabling is a person's act: doctor names the command and never runs it.
echo disabled >"$unit_state"
run doctor --fix
is "$RC" 1 'a unit systemd does not report enabled is a finding, --fix or not'
like "$OUT" 'unit workflow\.service.*systemctl --user enable --now workflow\.service' \
	'and the finding gives the command'

mkdir -p "$T_TMP/no-systemd"
ln -sf "$(command -v git)" "$T_TMP/no-systemd/git"
run env PATH="$T_TMP/no-systemd" "$wf" doctor
unlike "$OUT" 'enable --now' 'with no systemctl the enabled check is skipped'
echo enabled >"$unit_state"
