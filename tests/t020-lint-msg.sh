#!/usr/bin/env bash
# workflow lint-msg: the three tiers of spec §3, --string for branch names and
# PR bodies, and clearing a warn term with a lint-exception ruling (AC4).
source "$(dirname -- "$0")/lib.sh"
t_init

new_repo repo
mem_register

# hard <text> <desc> -- the provenance tier: exit 1, whatever else is in there.
hard() {
	run workflow lint-msg --string "$1"
	is "$RC" 1 "hard: $2"
}

# clean <text> <desc> -- exit 0 and nothing said about it.
clean() {
	run workflow lint-msg --string "$1"
	is "$RC" 0 "clean: $2 (exit 0)"
	unlike "$OUT" 'warn' "clean: $2 (silent)"
}

# warns <text> <desc> -- exit 0, but the term is named.
warns() {
	run workflow lint-msg --string "$1"
	is "$RC" 0 "warn: $2 (exit 0)"
	like "$OUT" 'warn' "warn: $2 (says so)"
}

hard 'Extract cart pricing

Co-Authored-By: Claude <noreply@anthropic.com>' 'a Claude co-author trailer'
hard 'Extract cart pricing

Generated with Claude Code' 'the generated-with line'
hard 'See https://claude.ai/code for the rest' 'a claude.ai/code link'
hard 'Extract cart pricing

Session: https://claude.ai/s/018f2c7e-0000-7000-8000-000000000000' 'a session URL'
hard 'Port the last of shipflow across' 'the shipflow name'
hard 'focus: extract cart pricing' 'a focus: subject prefix'
hard 'gate: tighten the pre-commit check' 'a gate: subject prefix'
hard 'track/cart-pricing' 'a track/ branch name'
hard 'Extract cart pricing 🤖' 'a robot emoji'
hard 'Extract cart pricing ✨' 'a sparkle emoji'

clean 'Extract cart pricing into a service' 'an ordinary subject'
clean 'Fix the checkout total when a coupon is applied

The rounding happened before the discount, so a 33% coupon on 100 gave 67
instead of 66.' 'an ordinary subject and body'
clean 'Refresh the retry backoff again' 'the word "again" is not the word AI'

warns 'Ignore orchestration scratch directory' 'the orchestrat* term'
warns 'Add an AI summary field to the product page' 'the bare word AI'
warns 'Refresh the user agent string' 'the word agent'
warns 'Note the LLM token ceiling in the README' 'the word LLM'
warns 'Answer the spec review comments' 'the phrase spec review'

# `AI` and `LLM` are matched case-sensitively; the ordinary English words are
# not. A deliberate asymmetry: an initialism is only process vocabulary when it
# is written as one, and matching it case-insensitively catches the name Ai, a
# variable called ai, and every language where it is an ordinary word.
clean 'Rename the column to ai_summary' 'a lowercase ai is not the initialism'
clean 'Credit the photograph to Ai Nakamura' 'nor is a name that happens to be Ai'
warns 'Refresh the User Agent string' 'while Agent still warns, agent being an ordinary word'
warns 'Note the ORCHESTRATION step in the README' 'and so does a shouted orchestration'

## --------------------------------------------------------------- style tier

warns 'Tidy the loop — again' 'an em dash'
warns 'Delving into the retry loop' 'the word delve, inflected'
warns 'In order to cut startup time' 'the phrase in order to'
clean 'Order the retries by age -- oldest first' 'a double hyphen is not an em dash'

"$MEM_BIN" save --kind ruling --type lint-exception \
	'"em dash" is deliberate here - the body quotes upstream text verbatim - cost if wrong: a leaked tell' >/dev/null
run workflow lint-msg --string 'Tidy the loop — again'
is "$RC" 0 'lint-exception: a cleared style term exits 0'
unlike "$OUT" 'warn' 'lint-exception: and is no longer warned about'
run workflow lint-msg --string 'Delving into the retry loop'
like "$OUT" 'warn' 'lint-exception: clears only the style term it names'

## ----------------------------------------------------- per-term exceptions

"$MEM_BIN" save --kind ruling --type lint-exception \
	'"orchestration" is this product'"'"'s own vocabulary - the scratch dir is named that - cost if wrong: a leaked word' >/dev/null
run workflow lint-msg --string 'Ignore orchestration scratch directory'
is "$RC" 0 'lint-exception: the cleared message exits 0'
unlike "$OUT" 'warn' 'lint-exception: the cleared term is no longer warned about'

run workflow lint-msg --string 'Refresh the user agent string'
like "$OUT" 'warn' 'lint-exception: clears only the term it names'

## ----------------------------------------------------------- file argument

printf 'Extract cart pricing into a service\n' >"$T_TMP/msg"
run workflow lint-msg "$T_TMP/msg"
is "$RC" 0 'msgfile: an ordinary message passes'

printf 'Extract cart pricing\n\nGenerated with Claude Code\n' >"$T_TMP/msg"
run workflow lint-msg "$T_TMP/msg"
is "$RC" 1 'msgfile: the provenance line fails'

# git hands the hook its own commentary, and with --verbose the whole diff.
cat >"$T_TMP/msg" <<'EOF'
Extract cart pricing into a service

# Please enter the commit message for your changes. Lines starting
# with '#' will be ignored, and an empty message aborts the commit.
#
# On branch track/cart-pricing
# Changes to be committed:
#	new file:   app/Services/Orchestration.php
# ------------------------ >8 ------------------------
# Do not modify or remove the line above.
diff --git a/x b/x
+Generated with Claude Code
EOF
run workflow lint-msg "$T_TMP/msg"
is "$RC" 0 'msgfile: git commentary and the scissors diff are not part of the message'
unlike "$OUT" 'warn' 'msgfile: nor do they raise warnings'

## ------------------------------------------------- branch names, PR bodies

run workflow lint-msg --string 'cart-pricing-v2'
is "$RC" 0 '--string: an ordinary branch name passes'
run workflow lint-msg --string 'gate:cart'
is "$RC" 1 '--string: a gate: branch name fails'
run workflow lint-msg --string '## Summary

Extracts cart pricing into a service.

🤖 Generated with Claude Code'
is "$RC" 1 '--string: a PR body carrying provenance fails'
