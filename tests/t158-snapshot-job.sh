#!/usr/bin/env bash
# The weekly snapshot job: mem snapshot, then rclone to Dropbox, the copy
# only if the snapshot succeeded. mem and rclone are stand-ins that log
# their argv; the unit files name the script and a weekly, persistent timer.
source "$(dirname -- "$0")/lib.sh"
t_init

units=$WF_ROOT/mem/units
bin=$T_TMP/standins
tarball=$T_TMP/snapshots/mem-20261017T000000Z.tar.gz
mkdir -p "$bin"
for tool in mem rclone; do
	cat >"$bin/$tool" <<'STANDIN'
#!/usr/bin/env bash
echo "${0##*/} $*" >>"$CALLS"
if [ "${0##*/}" = mem ] && [ "$1" = snapshot ]; then
	[ -n "${MEM_FAIL:-}" ] && exit 3
	echo "$TARBALL"
fi
if [ "${0##*/}" = rclone ]; then exit "${RCLONE_RC:-0}"; fi
exit 0
STANDIN
	chmod +x "$bin/$tool"
done
export TARBALL=$tarball CALLS=$T_TMP/calls

job() {
	: >"$CALLS"
	PATH=$bin:$PATH bash "$units/mem-snapshot.sh" >/dev/null 2>&1
	RC=$?
	OUT=$(cat "$CALLS")
}

## ------------------------------------------------------------ all succeed

RCLONE_RC=0 job
is "$RC" 0 'success: exits 0'
is "$OUT" "mem snapshot
rclone copy $tarball Dropbox:mem-snapshots/" 'success: snapshot, then copy that tarball'

## ------------------------------------------------------------ copy fails

RCLONE_RC=1 job
isnt "$RC" 0 'copy fails: exits non-zero'

## ------------------------------------------------------ snapshot fails

MEM_FAIL=1 job
isnt "$RC" 0 'snapshot fails: exits non-zero'
is "$OUT" "mem snapshot" 'snapshot fails: rclone never runs'

## ------------------------------------------------------------- units

svc=$(cat "$units/mem-snapshot.service")
tmr=$(cat "$units/mem-snapshot.timer")
like "$svc" '^ExecStart=%h/\.local/bin/mem-snapshot$' 'service runs the installed mem-snapshot script'
like "$svc" '^Type=oneshot$' 'service is oneshot'
like "$tmr" '^OnCalendar=weekly$' 'timer is weekly'
like "$tmr" '^Persistent=true$' 'timer is persistent'
