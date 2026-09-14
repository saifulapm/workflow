#!/usr/bin/env bash
# Run the workflow test suite.
#
#   tests/run.sh            # everything
#   tests/run.sh verify     # only tests whose filename contains "verify"
#
# Each test script is a separate process printing TAP-ish lines; this runner
# tallies them. WF_KEEP_TMP=1 leaves the sandboxes behind for inspection.
#
# Files run WF_JOBS at a time (default: half the machine's cores, at least
# two), each into its own sandbox (lib.sh t_init gives every file a throwaway
# HOME, so the suite lock a test takes is its own). Output is printed per
# file, in name order, once every file has ended: the 42 files took eight
# minutes in a row and the merge gate paid that per task. Half the cores, not
# all: a run test under heavier load than that saw mem's question listing
# lag past the run's three-poll tolerance and end a run over an answer that
# was on its way.

set -uo pipefail

cd -- "$(dirname -- "$0")/.." || exit 1
root=$PWD
pattern=${1:-}

# Built every time, not only when it is missing: a stale binary next to changed
# sources passes tests that the code it stands for would fail. CARGO_TARGET_DIR
# is pinned here rather than left to whatever the caller's shell has: a session
# running under the workflow already has one set for its own task, and that
# value would otherwise carry the binary off to a path this suite never checks.
mem_bin=${MEM_BIN:-$root/mem/target/release/mem}
if [ -z "${MEM_BIN:-}" ] || [ ! -x "$mem_bin" ]; then
	printf 'building mem (tests need %s)\n' "$mem_bin"
	(cd "$root/mem" && CARGO_TARGET_DIR="$root/mem/target" cargo build --release) || exit 1
	if [ ! -x "$mem_bin" ]; then
		printf 'mem build did not produce %s\n' "$mem_bin" >&2
		exit 1
	fi
fi
export MEM_BIN=$mem_bin

# Same rule for the program under test: when it is the crate's release binary,
# build it every run so a stale binary cannot pass for changed sources.
wf_bin=${WORKFLOW_BIN:-$root/workflow/target/release/workflow}
case "$wf_bin" in
*/workflow/target/release/workflow)
	printf 'building workflow (tests need %s)\n' "$wf_bin"
	(cd "$root/workflow" && CARGO_TARGET_DIR="$root/workflow/target" cargo build --release) || exit 1
	if [ ! -x "$wf_bin" ]; then
		printf 'workflow build did not produce %s\n' "$wf_bin" >&2
		exit 1
	fi
	;;
esac
export WORKFLOW_BIN=$wf_bin

total=0
failed=0
files=0
failed_files=()
failed_checks=""
cores=$(nproc 2>/dev/null || echo 4)
jobs=${WF_JOBS:-$((cores / 2 > 2 ? cores / 2 : 2))}
outs=$(mktemp -d "${TMPDIR:-/tmp}/wf-suite.XXXXXX")

selected=()
for t in "$root"/tests/t*.sh; do
	name=$(basename -- "$t")
	if [ -n "$pattern" ] && [[ $name != *"$pattern"* ]]; then continue; fi
	selected+=("$t")
done

running=0
for t in "${selected[@]}"; do
	name=$(basename -- "$t")
	(
		bash "$t" >"$outs/$name.out" 2>&1
		echo $? >"$outs/$name.status"
	) &
	running=$((running + 1))
	if [ "$running" -ge "$jobs" ]; then
		wait -n
		running=$((running - 1))
	fi
done
wait

for t in "${selected[@]}"; do
	name=$(basename -- "$t")
	files=$((files + 1))
	printf '\n== %s\n' "$name"
	out=$(cat "$outs/$name.out")
	status=$(cat "$outs/$name.status" 2>/dev/null || echo 1)
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
rm -rf "$outs"

printf '\n----\n%d files, %d checks, %d failed\n' "$files" "$total" "$failed"
if [ ${#failed_files[@]} -gt 0 ]; then
	printf 'failing files: %s\n' "${failed_files[*]}"
	printf '%s' "$failed_checks" | grep -v '^$'
fi
[ "$failed" -eq 0 ] || exit 1
