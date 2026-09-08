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

STORE="${PKG_CACHE_DIR:-${RUNNER_TEMP:-/tmp}/pkg-cache}"
REMOTE="$(cd "$(dirname "$0")" && pwd)/pkg-remote.sh"
INDEX="$STORE/../pkg-index"

# One fetch, before the build, of the list of keys the remote holds. The wrapper then answers a miss
# from that list instead of asking the service per key.
mkdir -p "$STORE"
index_rc=1
if [ -z "${PKG_NO_REMOTE:-}" ] && [ -x "$REMOTE" ]; then
	"$REMOTE" get pkg-index "$STORE/../pkg-index-dl"
	index_rc=$?
	[ "$index_rc" = 0 ] && cp "$STORE/../pkg-index-dl/keys" "$INDEX"
fi

start=$(date +%s)
cargo test --locked --workspace --no-run --no-fail-fast
rc=$?
wall=$(( $(date +%s) - start ))

# The uploads are detached from the compiles that made them, so the wall above is the compile and
# nothing else. They still have to land before the index names them.
drain_start=$(date +%s)
"$(cd "$(dirname "$0")" && pwd)/pkg-cache.sh" drain
drain=$(( $(date +%s) - drain_start ))

# The store's entry names are the keys, so publishing the index is listing it. A leg that never gets
# here leaves its entries unreachable, which the next leg reports as remote-no-index.
index_put=1
if [ -z "${PKG_NO_REMOTE:-}" ] && [ -x "$REMOTE" ]; then
	mkdir -p "$STORE/../pkg-index-up"
	find "$STORE" -maxdepth 1 -mindepth 1 -type d -printf '%f\n' > "$STORE/../pkg-index-up/keys"
	"$REMOTE" put pkg-index "$STORE/../pkg-index-up"
	index_put=$?
fi

say() { echo "MEASURE $TECHNIQUE $PHASE $*"; }
# A tally nothing incremented has no file. The redirection fails before wc runs, so wc's own stderr
# is the wrong place to silence it, and every leg printed an error per absent counter.
count() { [ -f "$STATS/$1" ] && wc -l < "$STATS/$1" || echo 0; }

say "salt ${PKG_CACHE_SALT:-none}"
# A timing that does not carry the knobs it ran under cannot be compared against another one.
say "config jobs=${CARGO_BUILD_JOBS:-default} slots=${PKG_SLOTS:-default} upload-slots=${PKG_UPLOAD_SLOTS:-default} wrapper=${WRAPPER:-none}"
say "wall $wall s rc=$rc"
# A leg that ran the disk out reads as a slow or dead compile unless the free space is on the record.
say "disk-free-kb $(df -Pk . | tail -1 | tr -s ' ' | cut -d' ' -f4)"
# Without these the remote layer cannot run at all, and every entry reads as an upload that failed.
say "cache-url-set ${ACTIONS_RESULTS_URL:+yes}${ACTIONS_RESULTS_URL:-no}"
say "cache-token-set ${ACTIONS_RUNTIME_TOKEN:+yes}${ACTIONS_RUNTIME_TOKEN:-no}"
say "binpazer $("${BINPAZER:-binpazer}" --version 2>/dev/null || echo MISSING)"
say "store $(du -sm "${PKG_CACHE_DIR:-/nonexistent}" 2>/dev/null | cut -f1) MB"
say "target $(du -sm target 2>/dev/null | cut -f1) MB"
say "upload-drain $drain s"
say "index-get rc=$index_rc entries=$(wc -l < "$INDEX" 2>/dev/null || echo 0)"
say "index-put rc=$index_put"
for c in local-hit remote-hit remote-miss remote-restore-failed remote-no-index remote-not-held compiled remote-put remote-put-failed remote-finalize-failed remote-unavailable remote-429; do
	say "$c $(count "$c")"
done

if [ "$(count remote-429)" -gt 0 ]; then
	say "FAILED: the cache service answered 429, so this timing measures the throttle"
	exit 1
fi
exit "$rc"
