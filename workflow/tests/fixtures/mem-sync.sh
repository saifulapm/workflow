# What `mem sync --json` prints, for test fakes: sh mem-sync.sh <outbox> [stuck].
# Shaped like mem/src/verbs.rs `sync`; tests/fake_mem_sync.rs holds it to
# mem/schemas/sync.json.
n=$1
if [ "${2:-}" = stuck ]; then
  printf '{"sync":"not performed","detail":"the store'"'"'s writes stopped landing here after sequence 9: a test said so","outbox":%s,"seq":9}\n' "$n"
elif [ "$n" -gt 0 ]; then
  printf '{"sync":"not performed","detail":"outbox %s, which the hub has not taken yet; last share sequence 9","outbox":%s,"seq":9}\n' "$n" "$n"
else
  printf '{"sync":"performed","detail":"outbox 0, last share sequence 9","outbox":0,"seq":9}\n'
fi
