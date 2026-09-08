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
# Only the transfers need the service. Printing a manifest does not, which is what lets a test
# check the packing rules off a runner.
if [ "$op" != manifest ]; then
	# This client speaks the v2 twirp API, which lives at ACTIONS_RESULTS_URL. A runner offering
	# only the v1 URL is a different protocol, not a missing one, and says so rather than reading
	# as an absent service.
	if [ -z "$BASE" ] && [ -n "${ACTIONS_CACHE_URL:-}" ]; then
		echo "pkg-remote: runner offers cache v1 only; this client speaks v2" >&2
	fi
	[ -n "$BASE" ] && [ -n "$TOKEN" ] || exit 3
fi

# The official client resolves /twirp against the base URL, which discards any path the base carries.
ORIGIN="$(printf '%s' "$BASE" | cut -d/ -f1-3)"
API="$ORIGIN/twirp/github.actions.results.api.v1.CacheService"

# binpazer, not tar: it carries a Block Index, so a reader seeks to one artifact instead of
# decompressing the whole member to reach it, and each block carries its own CRC and codec.
BINPAZER="${BINPAZER:-binpazer}"
TYPE_ARTIFACT=1
TYPE_NAMES=2

# The names block. binpazer stores payloads and does not model a file name, so the names travel as
# their own critical block, in the order the artifact blocks were written.
write_manifest() {
	local d="$1" f names_json=""
	local list=""
	for f in "$d"/*; do
		[ -f "$f" ] || continue
		list="$list$(basename "$f")
"
	done
	[ -n "$list" ] || return 1
	names_json="$(printf '%s' "$list" | jq -Rs .)"

	printf '{"writer_guid":"8f9d2c31-4b6a-4e0f-9a3d-1c2b3a4d5e6f","writer_name":"pkg-cache",'
	printf '"types":[{"type_id":1,"guid":"6ba7b810-9dad-11d1-80b4-00c04fd430c8","name":"Artifact"},'
	printf '{"type_id":2,"guid":"6ba7b811-9dad-11d1-80b4-00c04fd430c8","name":"Names"}],"blocks":['
	printf '{"type_id":2,"flags":["critical"],"data":%s}' "$names_json"
	for f in "$d"/*; do
		[ -f "$f" ] || continue
		printf ',{"type_id":1,"flags":["has_crc"],"codec":"zstd","file":%s}' "$(printf '%s' "$f" | jq -Rs .)"
	done
	printf '],"index":true}'
}

# The version field scopes a key to the archive format that wrote it. Changing the format must miss
# rather than restore a container this script cannot read.
#
# PKG_CACHE_SALT scopes it further. A measurement sets it per run, so the cold leg meets an empty
# keyspace and the warm leg behind it meets what that cold leg wrote. Without it a cold leg is cold
# exactly once, and every later one is served by an earlier run while still calling itself cold.
VERSION="$(printf 'pkg-cache-binpazer-v1%s' "${PKG_CACHE_SALT:-}" | sha256sum | cut -d' ' -f1)"

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
	# The body is the only thing that says WHY a call was refused, so a debug run keeps it.
	if [ -n "${PKG_REMOTE_DEBUG:-}" ] && [ "$code" != 200 ]; then
		echo "pkg-remote: $1 answered HTTP $code: ${body%$'\n'*}" >&2
	fi
	[ "$code" = 200 ] || return 1
	printf '%s' "${body%$'\n'*}"
}

case "$op" in
# Prints the manifest for a directory and stops. The packing rules are then checkable without a
# cache service, which is the only way a test can reach them.
manifest)
	write_manifest "$dir"
	exit $?
	;;
get)
	body="$(printf '{"key":"%s","restore_keys":[],"version":"%s"}' "$key" "$VERSION")"
	answer="$(rpc GetCacheEntryDownloadURL "$body")"
	rc=$?
	[ "$rc" = "$THROTTLED" ] && exit "$THROTTLED"
	url="$(printf '%s' "$answer" | jq -r 'select(.ok == true) | .signed_download_url // .signedDownloadUrl // empty')"
	[ -n "$url" ] || exit 1
	mkdir -p "$dir" || exit 1
	blob="$(mktemp)" || exit 1
	trap 'rm -f "$blob"' EXIT
	curl -fsS --max-time 300 "$url" -o "$blob" 2>/dev/null || exit 1
	# The names ride in their own block: binpazer stores payloads, and a file name is the caller's
	# business, not the format's.
	namefile="$blob.names"
	"$BINPAZER" extract "$blob" --type "$TYPE_NAMES" -o "$namefile" 2>/dev/null || exit 1
	i=0
	while IFS= read -r name; do
		[ -n "$name" ] || continue
		"$BINPAZER" extract "$blob" --type "$TYPE_ARTIFACT" --index "$i" -o "$dir/$name" 2>/dev/null || exit 1
		i=$((i + 1))
	done < "$namefile"
	rm -f "$namefile"
	[ "$i" -gt 0 ] || exit 1
	;;
put)
	tmp="$(mktemp)" || exit 1
	man="$(mktemp)" || exit 1
	trap 'rm -f "$tmp" "$man"' EXIT
	write_manifest "$dir" > "$man" || exit 1
	"$BINPAZER" pack "$man" -o "$tmp" >/dev/null 2>&1 || exit 1
	size="$(stat -c %s "$tmp")"

	body="$(printf '{"key":"%s","version":"%s"}' "$key" "$VERSION")"
	answer="$(rpc CreateCacheEntry "$body")"
	rc=$?
	[ "$rc" = "$THROTTLED" ] && exit "$THROTTLED"
	url="$(printf '%s' "$answer" | jq -r 'select(.ok == true) | .signed_upload_url // .signedUploadUrl // empty')"
	# A key another job already wrote answers not-ok. That is a hit, not a failure.
	if [ -z "$url" ]; then
		[ -n "${PKG_REMOTE_DEBUG:-}" ] && echo "pkg-remote: CreateCacheEntry gave no url: $answer" >&2
		exit 1
	fi

	# One Put Blob, not a block list. An entry is a few MB, far under the 256 MB single-shot limit,
	# and the block/blocklist pair was two chances to get a commit wrong for no gain.
	# PKG_REMOTE_DEBUG puts the transfer's own errors on the log, because a failed upload is
	# otherwise indistinguishable from a service nobody wired up.
	if [ -n "${PKG_REMOTE_DEBUG:-}" ]; then
		curl -fsS --max-time 300 -X PUT "$url" -H "x-ms-blob-type: BlockBlob" \
			--data-binary "@$tmp" >/dev/null || { echo "pkg-remote: upload failed for $key" >&2; exit 1; }
	else
		curl -fsS --max-time 300 -X PUT "$url" -H "x-ms-blob-type: BlockBlob" \
			--data-binary "@$tmp" >/dev/null 2>&1 || exit 1
	fi

	final="$(printf '{"key":"%s","size_bytes":%s,"version":"%s"}' "$key" "$size" "$VERSION")"
	rpc FinalizeCacheEntryUpload "$final" | jq -e '.ok == true' >/dev/null 2>&1 || exit 4
	;;
*) exit 2 ;;
esac
