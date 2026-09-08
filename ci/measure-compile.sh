#!/bin/bash
# Times one full workspace compile and reports what the cache served.
#
# The reader compares a cold leg against a warm one. remote-hit is what proves which a log is: a
# cold leg reporting hits was served by an earlier run and is not a cold measurement.
#
# A throttled cache service is a measurement of the throttle. remote-429 above zero therefore fails
# the leg, rather than letting the wait it caused read as a slow compile.
set -uo pipefail

: "${TECHNIQUE:?names the leg}"
: "${PHASE:?cold or warm}"
STATS="${PKG_STATS_DIR:-/tmp/pkg-stats}"

if [ -n "${WRAPPER:-}" ]; then
	export RUSTC_WRAPPER="${GITHUB_WORKSPACE:-$PWD}/$WRAPPER"
fi

start=$(date +%s)
cargo test --locked --workspace --no-run --no-fail-fast
rc=$?
wall=$(( $(date +%s) - start ))

say() { echo "MEASURE $TECHNIQUE $PHASE $*"; }
count() { wc -l < "$STATS/$1" 2>/dev/null || echo 0; }

say "salt ${PKG_CACHE_SALT:-none}"
say "wall $wall s rc=$rc"
say "store $(du -sm "${PKG_CACHE_DIR:-/nonexistent}" 2>/dev/null | cut -f1) MB"
say "target $(du -sm target 2>/dev/null | cut -f1) MB"
for c in local-hit remote-hit remote-miss compiled remote-put remote-put-failed remote-429; do
	say "$c $(count "$c")"
done

if [ "$(count remote-429)" -gt 0 ]; then
	say "FAILED: the cache service answered 429, so this timing measures the throttle"
	exit 1
fi
exit "$rc"
