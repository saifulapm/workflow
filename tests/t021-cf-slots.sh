#!/usr/bin/env bash
# t_cf_dev runs at most two cf dev servers at once on this machine, whatever
# WF_JOBS says: seven at once overflowed the mini's memory and failed 57
# checks. A third waits until one of the first two tests has ended.
source "$(dirname -- "$0")/lib.sh"
t_init

# A cf that serves a directory on the port it is given, in place of cf dev.
fake=$T_TMP/fakeweb
mkdir -p "$fake/node_modules/.bin" "$T_TMP/app"
cat >"$fake/node_modules/.bin/cf" <<'EOF'
#!/usr/bin/env bash
exec python3 -m http.server "$3" --bind 127.0.0.1
EOF
chmod +x "$fake/node_modules/.bin/cf"
export HUBWEB=$fake real_cache=$XDG_CACHE_HOME WF_CF_SLOTS=$T_TMP/slots

# One test: start cf dev, say so, hold it 3 s, then end as a test does.
one() {
	t_cf_dev "$T_TMP/app" "$T_TMP/cf$1.log" || exit 1
	touch "$T_TMP/up$1"
	sleep 3
	kill -TERM -- "-$CF_DEV_PID" 2>/dev/null
}
for i in 1 2 3; do (one "$i") & done

sleep 1.5
up=$(find "$T_TMP" -maxdepth 1 -name 'up*' | wc -l)
is "$up" 2 "two cf dev tests run at once, the third waits"

wait
up=$(find "$T_TMP" -maxdepth 1 -name 'up*' | wc -l)
is "$up" 3 "the third starts once one of the two has ended"
