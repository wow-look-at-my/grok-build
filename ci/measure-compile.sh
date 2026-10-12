#!/bin/bash
# Times one full workspace compile and reports what the cache served.
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
# The index key names the TECHNIQUE.
INDEXKEY="pkg-index-$TECHNIQUE"

# A leg with no wrapper has no store to index, so it neither fetches nor publishes one.
USE_INDEX=1
[ -n "${WRAPPER:-}" ] || USE_INDEX=""
[ -z "${PKG_NO_REMOTE:-}" ] || USE_INDEX=""
[ -x "$REMOTE" ] || USE_INDEX=""

# One fetch, before the build, of the list of keys the remote holds.
mkdir -p "$STORE"
# A fetch that fails must leave no index.
rm -f "$INDEX"
index_rc=1
if [ -n "$USE_INDEX" ]; then
	"$REMOTE" get "$INDEXKEY" "$STORE/../pkg-index-dl"
	index_rc=$?
	[ "$index_rc" = 0 ] && cp "$STORE/../pkg-index-dl/keys" "$INDEX"
fi

start=$(date +%s)
cargo test --locked --workspace --no-run --no-fail-fast
rc=$?
wall=$(( $(date +%s) - start ))

# The uploads are detached from the compiles that made them, so the wall above is the compile and nothing else.
drain_start=$(date +%s)
"$(cd "$(dirname "$0")" && pwd)/pkg-cache.sh" drain
drain=$(( $(date +%s) - drain_start ))

# The store's entry names are the keys, so publishing the index is listing it.
index_put=1
if [ -n "$USE_INDEX" ]; then
	mkdir -p "$STORE/../pkg-index-up"
	find "$STORE" -maxdepth 1 -mindepth 1 -type d -printf '%f\n' > "$STORE/../pkg-index-up/keys"
	"$REMOTE" put "$INDEXKEY" "$STORE/../pkg-index-up"
	index_put=$?
fi

say() { echo "MEASURE $TECHNIQUE $PHASE $*"; }
# A tally nothing incremented has no file.
count() { [ -f "$STATS/$1" ] && wc -l < "$STATS/$1" || echo 0; }

say "salt ${PKG_CACHE_SALT:-none}"
# A timing that does not carry the knobs it ran under cannot be compared against another.
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
for c in local-hit remote-hit remote-miss remote-restore-failed remote-no-index remote-not-held compiled remote-put remote-put-exists remote-put-failed remote-finalize-failed remote-unavailable remote-429 remote-429-retried; do
	say "$c $(count "$c")"
done

# A throttle the upload waited out and then stored is backpressure the drain
# paid for, and the wall above excludes it.
if [ "$(count remote-429)" -gt 0 ]; then
	say "FAILED: an upload gave up on a 429, so the cache is missing entries this timing assumes"
	exit 1
fi

# A warm leg exists to time what the cache serves. One that served nothing
# timed a cold build under a warm name, and it reported success: the counters
# said so and no check read them.
if [ -n "$USE_INDEX" ] && [ "$PHASE" = warm ] &&
	[ "$(( $(count remote-hit) + $(count local-hit) ))" = 0 ]; then
	say "FAILED: the warm leg served no entry, so it timed a cold build"
	exit 1
fi

# The warm leg reads this index. A cold leg that does not publish one leaves its partner nothing to
# find, which is exactly the run that produced the line above.
if [ -n "$USE_INDEX" ] && [ "$PHASE" = cold ] && [ "$index_put" != 0 ]; then
	say "FAILED: the index was not published, so the warm leg cannot reach this leg's entries"
	exit 1
fi
exit "$rc"
