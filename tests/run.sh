#!/usr/bin/env bash
# Run the workflow test suite.
#
#   tests/run.sh            # everything
#   tests/run.sh verify     # only tests whose filename contains "verify"
#
# Each test script is a separate process printing TAP-ish lines; this runner
# tallies them. WF_KEEP_TMP=1 leaves the sandboxes behind for inspection.

set -uo pipefail

cd -- "$(dirname -- "$0")/.." || exit 1
root=$PWD
pattern=${1:-}

# Built every time, not only when it is missing: a stale binary next to changed
# sources passes tests that the code it stands for would fail.
mem_bin=${MEM_BIN:-$root/mem/target/release/mem}
if [ -z "${MEM_BIN:-}" ] || [ ! -x "$mem_bin" ]; then
	printf 'building mem (tests need %s)\n' "$mem_bin"
	(cd "$root/mem" && cargo build --release) || exit 1
fi
export MEM_BIN=$mem_bin

# Same rule for the program under test: when it is the crate's release binary,
# build it every run so a stale binary cannot pass for changed sources.
wf_bin=${WORKFLOW_BIN:-$root/workflow/target/release/workflow}
case "$wf_bin" in
*/workflow/target/release/workflow)
	printf 'building workflow (tests need %s)\n' "$wf_bin"
	(cd "$root/workflow" && cargo build --release) || exit 1
	;;
esac
export WORKFLOW_BIN=$wf_bin

total=0
failed=0
files=0
failed_files=()
failed_checks=""

for t in "$root"/tests/t*.sh; do
	name=$(basename -- "$t")
	if [ -n "$pattern" ] && [[ $name != *"$pattern"* ]]; then continue; fi
	files=$((files + 1))
	printf '\n== %s\n' "$name"
	out=$(bash "$t" 2>&1)
	status=$?
	printf '%s\n' "$out"
	n_ok=$(printf '%s\n' "$out" | grep -c '^ok ')
	n_no=$(printf '%s\n' "$out" | grep -c '^not ok ')
	total=$((total + n_ok + n_no))
	failed=$((failed + n_no))
	if [ "$status" -ne 0 ]; then
		[ "$n_no" -eq 0 ] && failed=$((failed + 1))
		failed_files+=("$name")
		# Repeated in the tail summary: a red that scrolled away unnamed cannot
		# be pinned afterwards (friction #BJVD1YCD).
		failed_checks+=$(printf '%s\n' "$out" | grep '^not ok ' | sed "s/^/$name: /")$'\n'
	fi
done

printf '\n----\n%d files, %d checks, %d failed\n' "$files" "$total" "$failed"
if [ ${#failed_files[@]} -gt 0 ]; then
	printf 'failing files: %s\n' "${failed_files[*]}"
	printf '%s' "$failed_checks" | grep -v '^$'
fi
[ "$failed" -eq 0 ] || exit 1
