#!/usr/bin/env bash
# A test run alone refuses a release binary older than its crate's sources.
#
# Builds nothing: a throwaway tree holds a copy of lib.sh, tiny fake crates and
# fake binaries, and a one-line test that sources lib.sh and calls t_init.
# touch -d sets which side is newer.

source "$(dirname -- "$0")/lib.sh"
t_init

tree=$T_TMP/tree
mkdir -p "$tree/tests"
cp "$TESTS_DIR/lib.sh" "$tree/tests/lib.sh"
for c in mem workflow hub hubpages; do
	mkdir -p "$tree/$c/src"
	: >"$tree/$c/src/x.rs"
	: >"$tree/$c/Cargo.toml"
done
printf 'hubpages = { path = "../hubpages" }\n' >"$tree/hub/Cargo.toml"
for c in mem workflow hub; do
	mkdir -p "$tree/$c/target/release"
	printf '#!/bin/sh\nexit 0\n' >"$tree/$c/target/release/$c"
	chmod +x "$tree/$c/target/release/$c"
done
# HUB_BIN is named here so the guard treats this as a hub test.
cat >"$tree/tests/tfake.sh" <<'FAKE'
#!/usr/bin/env bash
source "$(dirname -- "$0")/lib.sh"
t_init
HUB_BIN=${HUB_BIN:-$WF_ROOT/hub/target/release/hub}
ok "ran"
FAKE

run_fake() { env -u MEM_BIN -u WORKFLOW_BIN -u HUB_BIN bash "$tree/tests/tfake.sh" 2>&1; }

old='2020-01-01 00:00:00'
new='2030-01-01 00:00:00'
# Every file old, every binary newer than all of them.
fresh_all() {
	find "$tree" -type f -exec touch -d "$old" {} +
	local c
	for c in mem workflow hub; do touch -d "$new" "$tree/$c/target/release/$c"; done
}

fresh_all
out=$(run_fake); st=$?
is "$st" 0 "fresh binaries: the test runs"
case "$out" in *"ok 1 - ran"*) ok "fresh binaries: the checks run" ;; *) notok "fresh binaries: the checks run" "$out" ;; esac

for c in mem workflow hub; do
	fresh_all
	touch -d "$new" "$tree/$c/src/x.rs"
	touch -d "$old" "$tree/$c/target/release/$c"
	out=$(run_fake); st=$?
	if [ "$st" -ne 0 ]; then ok "$c source newer than its binary: refused"; else notok "$c source newer than its binary: refused" "$out"; fi
	case "$out" in
	*"cd $tree/$c && cargo build --release"*) ok "$c: names the build command" ;;
	*) notok "$c: names the build command" "$out" ;;
	esac
	case "$out" in *"ok 1 - ran"*) notok "$c: no check runs" "$out" ;; *) ok "$c: no check runs" ;; esac
done

fresh_all
touch -d "$new" "$tree/mem/Cargo.toml"
touch -d "$old" "$tree/mem/target/release/mem"
out=$(run_fake); st=$?
if [ "$st" -ne 0 ]; then ok "newer Cargo.toml: refused"; else notok "newer Cargo.toml: refused" "$out"; fi

fresh_all
touch -d "$new" "$tree/hubpages/src/x.rs"
touch -d "$old" "$tree/hub/target/release/hub"
out=$(run_fake); st=$?
if [ "$st" -ne 0 ]; then ok "newer path dependency source: hub refused"; else notok "newer path dependency source: hub refused" "$out"; fi

fresh_all
touch -d "$new" "$tree/mem/src/x.rs"
touch -d "$old" "$tree/mem/target/release/mem"
cp "$tree/mem/target/release/mem" "$tree/elsewhere"
out=$(env -u WORKFLOW_BIN -u HUB_BIN MEM_BIN="$tree/elsewhere" bash "$tree/tests/tfake.sh" 2>&1); st=$?
is "$st" 0 "a MEM_BIN outside the release path is left alone"
