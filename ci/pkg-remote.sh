#!/bin/bash
# The remote half of the per-package cache: one cache entry per package.
#
# A runner is a fresh VM, so a store under RUNNER_TEMP is cold on every run. This puts each
# package's artifacts in the Actions cache service under that package's own key, which is what
# makes a later run warm.
#
# One entry per package is the point. Holding each object separately measured 2915 fetches and
# about 607 s of latency for this workspace. Holding the whole store as one entry does not work
# either: a cache entry is immutable, so a key that names only the lockfile is written one time
# and frozen while the store keeps growing.
#
#   pkg-remote.sh get <key> <destdir>   restores into destdir, exit 0 only when it restored
#   pkg-remote.sh put <key> <srcdir>    stores srcdir, exit 0 when stored
#
# Outside Actions there is no cache service, so both answer non-zero and the caller compiles.
set -uo pipefail

op="${1:-}"
key="${2:-}"
dir="${3:-}"
[ -n "$op" ] && [ -n "$key" ] && [ -n "$dir" ] || exit 2

BASE="${ACTIONS_RESULTS_URL:-}"
TOKEN="${ACTIONS_RUNTIME_TOKEN:-}"
[ -n "$BASE" ] && [ -n "$TOKEN" ] || exit 3

# The official client resolves /twirp against the base URL, which discards any path the base carries.
ORIGIN="$(printf '%s' "$BASE" | cut -d/ -f1-3)"
API="$ORIGIN/twirp/github.actions.results.api.v1.CacheService"

# The version field scopes a key to the archive format that wrote it. Changing the format must miss
# rather than restore a tarball this script cannot read.
VERSION="$(printf 'pkg-cache-tar-zstd-v1' | sha256sum | cut -d' ' -f1)"

# A 429 is reported, never folded into the miss path: a throttled fetch reads as a slow compile, and
# that is the one failure a timing run must not absorb quietly.
THROTTLED=9
rpc() {
	local body code
	body="$(curl -sS --max-time 60 -X POST "$API/$1" \
		-H "Authorization: Bearer $TOKEN" \
		-H "Content-Type: application/json" \
		-d "$2" -w '\n%{http_code}' 2>/dev/null)"
	code="${body##*$'\n'}"
	if [ "$code" = 429 ]; then
		echo "pkg-remote: the cache service answered 429 on $1" >&2
		return "$THROTTLED"
	fi
	[ "$code" = 200 ] || return 1
	printf '%s' "${body%$'\n'*}"
}

case "$op" in
get)
	body="$(printf '{"key":"%s","restore_keys":[],"version":"%s"}' "$key" "$VERSION")"
	answer="$(rpc GetCacheEntryDownloadURL "$body")"
	rc=$?
	[ "$rc" = "$THROTTLED" ] && exit "$THROTTLED"
	url="$(printf '%s' "$answer" | jq -r 'select(.ok == true) | .signed_download_url // .signedDownloadUrl // empty')"
	[ -n "$url" ] || exit 1
	mkdir -p "$dir" || exit 1
	curl -fsS --max-time 300 "$url" 2>/dev/null | tar -x --zstd -C "$dir" 2>/dev/null || exit 1
	;;
put)
	tmp="$(mktemp)" || exit 1
	trap 'rm -f "$tmp"' EXIT
	tar -c --zstd -C "$dir" . > "$tmp" 2>/dev/null || exit 1
	size="$(stat -c %s "$tmp")"

	body="$(printf '{"key":"%s","version":"%s"}' "$key" "$VERSION")"
	answer="$(rpc CreateCacheEntry "$body")"
	rc=$?
	[ "$rc" = "$THROTTLED" ] && exit "$THROTTLED"
	url="$(printf '%s' "$answer" | jq -r 'select(.ok == true) | .signed_upload_url // .signedUploadUrl // empty')"
	# A key another job already wrote answers not-ok. That is a hit, not a failure.
	[ -n "$url" ] || exit 1

	curl -fsS --max-time 300 -X PUT "$url&comp=block&blockid=$(printf 'block0' | base64 -w0)" \
		-H "x-ms-blob-type: BlockBlob" \
		--data-binary "@$tmp" >/dev/null 2>&1 || exit 1
	curl -fsS --max-time 60 -X PUT "$url&comp=blocklist" \
		-H "Content-Type: application/xml" \
		--data "<?xml version=\"1.0\" encoding=\"utf-8\"?><BlockList><Latest>$(printf 'block0' | base64 -w0)</Latest></BlockList>" \
		>/dev/null 2>&1 || exit 1

	final="$(printf '{"key":"%s","size_bytes":%s,"version":"%s"}' "$key" "$size" "$VERSION")"
	rpc FinalizeCacheEntryUpload "$final" | jq -e '.ok == true' >/dev/null 2>&1 || exit 1
	;;
*) exit 2 ;;
esac
