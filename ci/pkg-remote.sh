#!/bin/bash
# The remote half of the per-package cache: one cache entry per package.
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
	# This client speaks the v2 twirp API, which lives at ACTIONS_RESULTS_URL.
	[ -n "$TOKEN" ] || exit 3
	[ -n "$BASE" ] || [ -n "${ACTIONS_CACHE_URL:-}" ] || exit 3
fi

# The official client resolves /twirp against the base URL, which discards any path the base carries.
ORIGIN="$(printf '%s' "$BASE" | cut -d/ -f1-3)"
API="$ORIGIN/twirp/github.actions.results.api.v1.CacheService"

# ACTIONS_CACHE_SERVICE_V2 is unset on this runner, which by the toolkit's own rule means v1.
V1_BASE="${ACTIONS_CACHE_URL:-}"
[ -n "$V1_BASE" ] && V1_BASE="${V1_BASE%/}/"
if [ -z "$BASE" ] && [ -n "$V1_BASE" ]; then
	USE_V1=1
else
	USE_V1=
fi

# v1 reserve, upload, commit. A reserve that is refused means the key already exists, which is a hit.
v1_put() {
	local reserved id
	reserved="$(curl -sS --max-time 60 -X POST "${V1_BASE}_apis/artifactcache/caches" \
		-H "Authorization: Bearer $TOKEN" -H "Accept: application/json;api-version=6.0-preview.1" \
		-H "Content-Type: application/json" \
		-d "$(printf '{"key":"%s","version":"%s","cacheSize":%s}' "$key" "$VERSION" "$3")" 2>/dev/null)"
	id="$(printf '%s' "$reserved" | jq -r '.cacheId // empty')"
	if [ -z "$id" ]; then
		[ -n "${PKG_REMOTE_DEBUG:-}" ] && echo "pkg-remote: v1 reserve refused: $reserved" >&2
		return 1
	fi
	curl -fsS --max-time 300 -X PATCH "${V1_BASE}_apis/artifactcache/caches/$id" \
		-H "Authorization: Bearer $TOKEN" -H "Accept: application/json;api-version=6.0-preview.1" \
		-H "Content-Type: application/octet-stream" \
		-H "Content-Range: bytes 0-$(($3 - 1))/*" --data-binary "@$2" >/dev/null 2>&1 || return 1
	curl -fsS --max-time 60 -X POST "${V1_BASE}_apis/artifactcache/caches/$id" \
		-H "Authorization: Bearer $TOKEN" -H "Accept: application/json;api-version=6.0-preview.1" \
		-H "Content-Type: application/json" \
		-d "$(printf '{"size":%s}' "$3")" >/dev/null 2>&1 || return 4
}

v1_get_url() {
	curl -sS --max-time 60 -G "${V1_BASE}_apis/artifactcache/cache" \
		-H "Authorization: Bearer $TOKEN" -H "Accept: application/json;api-version=6.0-preview.1" \
		--data-urlencode "keys=$key" --data-urlencode "version=$VERSION" 2>/dev/null |
		jq -r '.archiveLocation // empty'
}

# binpazer, not tar: it carries a Block Index, so a reader seeks to one artifact instead of decompressing the whole member to reach it.
BINPAZER="${BINPAZER:-binpazer}"
TYPE_ARTIFACT=1
TYPE_NAMES=2

# The names block. binpazer stores payloads. Binpazer does not model a file name or a permission,
# so both travel as their own critical block. In the order the artifact blocks were written.
#
# The mode rides with the name because a package's artifact set includes the build script's own
# binary. Restored without its execute bit, cargo answers "could not execute process ... (never
# executed) ... Permission denied" and the build dies on a hit. A local hit hardlinks and never lost
# it, so this reaches only the entries that come back from the service.
write_manifest() {
	local d="$1" f names_json=""
	local list=""
	for f in "$d"/*; do
		[ -f "$f" ] || continue
		list="$list$(stat -c %a "$f") $(basename "$f")
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
		printf ',{"type_id":1,"flags":["has_crc"],"codec":"zstd","file":%s}' \
			"$(printf '%s' "$(basename "$f")" | jq -Rs .)"
	done
	printf '],"index":true}'
}

# The version field scopes a key to the archive format that wrote it.
VERSION="$(printf 'pkg-cache-binpazer-v2%s' "${PKG_CACHE_SALT:-}" | sha256sum | cut -d' ' -f1)"

THROTTLED=9
# The service already holds this key.
EXISTS=5
THROTTLE_WAIT="${PKG_THROTTLE_WAIT:-2}"
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
# Prints the manifest for a directory and stops.
manifest)
	write_manifest "$dir"
	exit $?
	;;
get)
	# Same reason as the put below: a throttled fetch is a compile the warm leg
	# was not supposed to do. A leg that recompiles is measuring different work
	# from the one it is compared against.
	while :; do
		"$0" get_once "$key" "$dir"
		rc=$?
		[ "$rc" = "$THROTTLED" ] || break
		[ -n "${PKG_STATS_DIR:-}" ] && echo x >> "$PKG_STATS_DIR/remote-429-retried" 2>/dev/null
		sleep "$THROTTLE_WAIT"
	done
	exit "$rc"
	;;
get_once)
	if [ -n "$USE_V1" ]; then
		url="$(v1_get_url)"
	else
		body="$(printf '{"key":"%s","restore_keys":[],"version":"%s"}' "$key" "$VERSION")"
		answer="$(rpc GetCacheEntryDownloadURL "$body")"
		rc=$?
		[ "$rc" = "$THROTTLED" ] && exit "$THROTTLED"
		url="$(printf '%s' "$answer" | jq -r 'select(.ok == true) | .signed_download_url // .signedDownloadUrl // empty')"
	fi
	[ -n "$url" ] || exit 1
	mkdir -p "$dir" || exit 1
	blob="$(mktemp)" || exit 1
	trap 'rm -f "$blob"' EXIT
	curl -fsS --max-time 300 "$url" -o "$blob" 2>/dev/null || exit 1
	# The names ride in their own block: binpazer stores payloads.
	namefile="$blob.names"
	"$BINPAZER" extract "$blob" --type "$TYPE_NAMES" -o "$namefile" 2>/dev/null || exit 1
	i=0
	# Each line is the mode, a space, then the name. A name may hold spaces, a mode may not, so the
	# split is on the FIRST space only.
	while IFS= read -r line; do
		[ -n "$line" ] || continue
		mode="${line%% *}"
		name="${line#* }"
		"$BINPAZER" extract "$blob" --type "$TYPE_ARTIFACT" --index "$i" -o "$dir/$name" 2>/dev/null || exit 1
		chmod "$mode" "$dir/$name" || exit 1
		i=$((i + 1))
	done < "$namefile"
	rm -f "$namefile"
	[ "$i" -gt 0 ] || exit 1
	;;
put)
	throttles=0
	while :; do
		"$0" put_once "$key" "$dir"
		rc=$?
		[ "$rc" = "$THROTTLED" ] || break
		throttles=$((throttles + 1))
		[ -n "${PKG_STATS_DIR:-}" ] && echo x >> "$PKG_STATS_DIR/remote-429-retried" 2>/dev/null
		sleep "$THROTTLE_WAIT"
	done
	exit "$rc"
	;;
put_once)
	tmp="$(mktemp)" || exit 1
	# binpazer resolves a manifest's file paths against the manifest's own directory.
	man="$dir/.pkg-manifest.json"
	trap 'rm -f "$tmp" "$man"' EXIT
	write_manifest "$dir" > "$man" || exit 1
	"$BINPAZER" pack "$man" -o "$tmp" >/dev/null 2>&1 || exit 1
	size="$(stat -c %s "$tmp")"

	if [ -n "$USE_V1" ]; then
		v1_put "$key" "$tmp" "$size"
		exit $?
	fi

	body="$(printf '{"key":"%s","version":"%s"}' "$key" "$VERSION")"
	answer="$(rpc CreateCacheEntry "$body")"
	rc=$?
	[ "$rc" = "$THROTTLED" ] && exit "$THROTTLED"
	url="$(printf '%s' "$answer" | jq -r 'select(.ok == true) | .signed_upload_url // .signedUploadUrl // empty')"
	# A key another job already wrote answers not-ok.
	if [ -z "$url" ]; then
		[ -n "${PKG_REMOTE_DEBUG:-}" ] && echo "pkg-remote: CreateCacheEntry gave no url: $answer" >&2
		exit "$EXISTS"
	fi

	# One Put Blob, not a block list. PKG_REMOTE_DEBUG puts the transfer's own errors on the log,
	# because a failed upload is otherwise indistinguishable from a service nobody wired up.
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
