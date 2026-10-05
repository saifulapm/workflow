#!/usr/bin/env bash
# The Claude Code plugin under plugin/: `claude plugin validate` accepts its
# manifest and hooks module, and `claude plugin test` passes its tests. With
# no claude 2.1.287 or newer installed, both are skipped.
source "$(dirname -- "$0")/lib.sh"
# claude is looked up on the caller's PATH, which t_init narrows.
claude_bin=$(command -v claude 2>/dev/null)
t_init

# 2.1.287 is the first build with function hooks and `claude plugin test`.
version=$([ -n "$claude_bin" ] && "$claude_bin" --version 2>/dev/null | sed -n '1s/^\([0-9][0-9.]*\).*/\1/p')
if [ -z "$version" ] || [ "$(printf '%s\n' 2.1.287 "$version" | sort -V | head -n 1)" != 2.1.287 ]; then
	printf '# skip: claude 2.1.287 or newer is not installed\n'
	exit 0
fi

cd "$WF_ROOT" || exit 1
run "$claude_bin" plugin validate plugin
is "$RC" 0 'claude plugin validate passes'
like "$OUT" 'hooks: session\.start, turn\.start, turn\.complete' 'and lists the three hooks'

run "$claude_bin" plugin test plugin
is "$RC" 0 'claude plugin test passes'
unlike "$OUT" '\(fail\)' 'and no test fails'
