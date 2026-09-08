#!/bin/bash
# A RUSTC_WRAPPER that caches per package and rations only the compiles it cannot serve.
#
# Cargo runs `$RUSTC_WRAPPER <rustc> <args...>`. This owns that call, so unlike a third-party
# compilation cache it knows whether it is about to compile. A hit copies artifacts back and takes
# no slot. A miss takes one of PKG_SLOTS flock slots and then compiles. Cargo's own -j can
# therefore stay wide: it paces restores, which cost no memory, while the semaphore paces compiles.
#
# The key is the package's identity, not the file's. It covers the rustc version, the command line
# with the varying output paths removed, and the crate's source content. A registry crate carries
# its version in an immutable path, so the path stands in for its content. A workspace crate is
# hashed, because it changes between runs and a key that misses that serves stale artifacts.
set -uo pipefail

REAL="$1"
shift

STORE="${PKG_CACHE_DIR:-${RUNNER_TEMP:-/tmp}/pkg-cache}"
SLOTS="${PKG_SLOTS:-3}"
SLOTDIR="${PKG_SLOT_DIR:-${RUNNER_TEMP:-/tmp}/pkg-slots}"
mkdir -p "$STORE" "$SLOTDIR"

# A probe reads a value out of the compiler. It writes no artifact, so there is nothing to cache
# and nothing to ration.
for a in "$@"; do
	case "$a" in
	--print | --print=* | -vV | --version) exec "$REAL" "$@" ;;
	esac
done

out_dir=""
crate_src=""
suffix=""
prev=""
key_args=()
for a in "$@"; do
	case "$prev" in
	--out-dir) out_dir="$a"; prev=""; continue ;;
	-C) case "$a" in extra-filename=*) suffix="${a#extra-filename=}" ;; esac
		key_args+=(-C "$a"); prev=""; continue ;;
	esac
	case "$a" in
	--out-dir) prev="--out-dir"; continue ;;
	-C) prev="-C"; continue ;;
	-Cextra-filename=*) suffix="${a#-Cextra-filename=}"; key_args+=("$a") ;;
	*.rs) crate_src="$a"; key_args+=("$a") ;;
	*) key_args+=("$a") ;;
	esac
done

# Every crate shares one --out-dir, so the directory is not this call's artifact set. What this call
# wrote is what carries its extra-filename, which is how cargo tells one crate's outputs from
# another's in there. Without that suffix the outputs cannot be told apart, so nothing is cached.
if [ -z "$out_dir" ] || [ -z "$crate_src" ] || [ -z "$suffix" ]; then
	exec "$REAL" "$@"
fi

# A registry source is immutable at its version, so its path identifies its content. Anything else
# is hashed, which is what keeps a workspace edit from hitting the previous run's artifacts.
#
# Every file counts, not only *.rs: `include_str!` reads a sibling of any name. A build script's
# generated code is reached by `include!` under $OUT_DIR, which is a directory outside the crate,
# so it is hashed as well. A key that misses either one serves an artifact of the older source.
# `target` and `.git` are pruned. A build script's crate directory is the package root, and a
# package that holds its own target directory otherwise hashes the output of the build it is part of.
hash_tree() {
	find "$1" \( -name target -o -name .git \) -prune -o -type f -print0 2>/dev/null |
		sort -z | xargs -0 -r sha256sum 2>/dev/null | sha256sum
}
crate_dir="$(dirname "$crate_src")"
case "$crate_dir" in
*/registry/src/*) content="$crate_dir" ;;
*) content="$(hash_tree "$crate_dir")" ;;
esac
if [ -n "${OUT_DIR:-}" ] && [ -d "${OUT_DIR:-}" ]; then
	content="$content $(hash_tree "$OUT_DIR")"
fi

key="$(printf '%s\0' "$("$REAL" -vV)" "${key_args[@]}" "$content" | sha256sum | cut -d' ' -f1)"
entry="$STORE/$key"

# Counted, not silent: a remote layer that quietly stops answering looks exactly like a slow build.
STATS="${PKG_STATS_DIR:-$STORE/../pkg-stats}"
mkdir -p "$STATS" 2>/dev/null
tally() { echo x >> "$STATS/$1" 2>/dev/null; }

# An empty entry must never read as a hit. Restoring nothing and reporting success hands cargo a
# missing artifact, which is a worse failure than a miss because it looks like a compiler bug.
restore() {
	[ -d "$1" ] && [ -n "$(ls -A "$1" 2>/dev/null)" ] || return 1
	cp -a "$1"/. "$out_dir"/ 2>/dev/null
}

if restore "$entry"; then
	tally local-hit
	exit 0
fi

# A runner is a fresh VM, so the local store is empty on the first build of a run. This is where a
# later run gets its warmth from.
REMOTE="$(dirname "$0")/pkg-remote.sh"
# PKG_NO_REMOTE keeps a measurement honest: entries an earlier run uploaded make a cold pass warm.
[ -n "${PKG_NO_REMOTE:-}" ] && REMOTE=""
if [ -n "$REMOTE" ] && [ -x "$REMOTE" ]; then
	if "$REMOTE" get "$key" "$entry" 2>/dev/null && restore "$entry"; then
		tally remote-hit
		exit 0
	fi
	tally remote-miss
fi

# A miss from here on, so it waits for a slot before it compiles.
BUSY=99
STATUS="$(mktemp)"
trap 'rm -f "$STATUS"' EXIT
while :; do
	for ((i = 0; i < SLOTS; i++)); do
		flock --nonblock --conflict-exit-code "$BUSY" "$SLOTDIR/slot.$i" \
			bash -c 'st=0; "$@" || st=$?; printf "%s\n" "$st" > "$0"; exit 0' \
			"$STATUS" "$REAL" "$@"
		if [ $? -ne "$BUSY" ]; then
			code=""
			read -r code < "$STATUS" || true
			[ -z "$code" ] && exit 2
			# Only a compile that succeeded may be served to a later run.
			if [ "$code" = 0 ]; then
				tally compiled
				tmp="$entry.$$"
				# Storing an empty directory is what makes a later run restore nothing and call it
				# a hit, so a set that matched no output is thrown away instead.
				if mkdir -p "$tmp" &&
					find "$out_dir" -maxdepth 1 -name "*$suffix*" -exec cp -a {} "$tmp"/ \; 2>/dev/null &&
					[ -n "$(ls -A "$tmp" 2>/dev/null)" ] &&
					mv -T "$tmp" "$entry" 2>/dev/null; then
					if [ -n "$REMOTE" ] && [ -x "$REMOTE" ]; then
						"$REMOTE" put "$key" "$entry" 2>/dev/null && tally remote-put || tally remote-put-failed
					fi
				else
					rm -rf "$tmp"
				fi
			fi
			exit "$code"
		fi
	done
	sleep 0.05
done
