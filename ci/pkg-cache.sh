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
UPLOADS="${PKG_UPLOAD_SLOTS:-8}"
SLOTDIR="${PKG_SLOT_DIR:-${RUNNER_TEMP:-/tmp}/pkg-slots}"
mkdir -p "$STORE" "$SLOTDIR"
# The locks are opened read-only, so they have to exist before anybody waits on one.
for ((_s = 0; _s < SLOTS; _s++)); do
	[ -e "$SLOTDIR/slot.$_s" ] || : >> "$SLOTDIR/slot.$_s"
done
for ((_u = 0; _u < UPLOADS; _u++)); do
	[ -e "$SLOTDIR/up.$_u" ] || : >> "$SLOTDIR/up.$_u"
done
[ -e "$SLOTDIR/uploads" ] || : >> "$SLOTDIR/uploads"

# Uploads outlive the wrapper calls that started them, so the job waits here before it ends. Each
# upload holds the shared lock; the exclusive one is granted only once every upload has released.
if [ "$REAL" = drain ]; then
	flock -x "$SLOTDIR/uploads" true
	exit 0
fi

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
# Memoised on the directory. Hashing one crate's sources measured 0.9 s, and a workspace build makes
# thousands of rustc calls over the same directories, so recomputing it per call costs more than the
# compiles this cache exists to skip. Sources cannot change while a build runs, so the memo is safe
# for the life of the store.
MEMO="$STORE/../pkg-hashes"
mkdir -p "$MEMO" 2>/dev/null
#
# The memo is named by substitution rather than by a hash of the path, and read with the shell's
# own read: both spend no process. A wrapper runs thousands of times in one build, so a fork it
# takes per call is time the compiles do not get.
hash_tree() {
	local memo cached
	memo="$MEMO/${1//\//%}"
	if [ -s "$memo" ]; then
		read -r cached < "$memo"
		printf '%s' "$cached"
		return
	fi
	local h
	h="$(find "$1" \( -name target -o -name .git \) -prune -o -type f -print0 2>/dev/null |
		sort -z | xargs -0 -r sha256sum 2>/dev/null | sha256sum)"
	printf '%s' "$h" > "$memo.$$" 2>/dev/null && mv -f "$memo.$$" "$memo" 2>/dev/null
	printf '%s' "$h"
}
crate_dir="${crate_src%/*}"
case "$crate_dir" in
*/registry/src/*) content="$crate_dir" ;;
*) content="$(hash_tree "$crate_dir")" ;;
esac
if [ -n "${OUT_DIR:-}" ] && [ -d "${OUT_DIR:-}" ]; then
	content="$content $(hash_tree "$OUT_DIR")"
fi

# Memoised for the same reason: one fork per rustc call, for a string that cannot change mid-build.
VERFILE="$MEMO/rustc-version"
if [ -s "$VERFILE" ]; then
	IFS= read -rd '' rustc_version < "$VERFILE"
else
	rustc_version="$("$REAL" -vV)"
	printf '%s' "$rustc_version" > "$VERFILE.$$" 2>/dev/null && mv -f "$VERFILE.$$" "$VERFILE" 2>/dev/null
fi

key="$(printf '%s\0' "$rustc_version" "${key_args[@]}" "$content" | sha256sum)"
key="${key%% *}"
entry="$STORE/$key"

# Counted, not silent: a remote layer that quietly stops answering looks exactly like a slow build.
STATS="${PKG_STATS_DIR:-$STORE/../pkg-stats}"
mkdir -p "$STATS" 2>/dev/null
tally() { echo x >> "$STATS/$1" 2>/dev/null; }

# An empty entry must never read as a hit. Restoring nothing and reporting success hands cargo a
# missing artifact, which is a worse failure than a miss because it looks like a compiler bug.
# Hardlinks, not copies. The store and target sit on one filesystem, and target measures 60 GB on a
# cold build, so a second copy of every package's artifacts is both the disk and the I/O this cache
# was meant to save. A rust artifact is written once and never edited, so sharing the inode is safe.
restore() {
	local -a have=("$1"/*)
	[ -e "${have[0]}" ] || return 1
	cp -al "$1"/. "$out_dir"/ 2>/dev/null || cp -a "$1"/. "$out_dir"/ 2>/dev/null
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
# The index says which keys the remote actually holds, so a miss costs no round trip. Asking the
# service per key made every miss a network wait, and a cold pass is nothing but misses: 2543 calls
# against an empty keyspace measured 348 s of pure asking.
#
# No index means the remote holds nothing this pass can use, and the gets are skipped rather than
# spent. A leg that died before publishing its index therefore leaves its entries unreachable to the
# next one. That is a miss, never a wrong answer, and it is counted so it cannot pass unseen.
INDEX="$STORE/../pkg-index"
may_get=1
if [ -z "$REMOTE" ] || [ ! -x "$REMOTE" ]; then
	may_get=""
elif [ ! -f "$INDEX" ]; then
	may_get=""
	tally remote-no-index
elif ! grep -qxF "$key" "$INDEX"; then
	may_get=""
	tally remote-not-held
fi
if [ -n "$REMOTE" ] && [ -x "$REMOTE" ] && [ -n "$may_get" ]; then
	"$REMOTE" get "$key" "$entry"
	got=$?
	# A throttle is not a miss. Counting it as one hides the wait inside the compile time.
	if [ "$got" = 9 ]; then
		tally remote-429
	elif [ "$got" = 3 ]; then
		tally remote-unavailable
	elif [ "$got" = 0 ] && restore "$entry"; then
		tally remote-hit
		exit 0
	else
		tally remote-miss
	fi
fi

# A miss from here on, so it waits for a slot before it compiles.
#
# The wait BLOCKS on one lock rather than polling a set of them. Polling forked flock once per slot
# every 50 ms per waiting call, and a wide outer -j leaves most calls waiting, so the semaphore was
# spending the two cores that the compiles it paces need.
#
# Blocking needs one lock per slot to still admit SLOTS at a time, so the slot is picked by this
# process id. That spreads callers evenly without anybody reading the other slots.
STATUS="$SLOTDIR/status.$$"
trap 'rm -f "$STATUS"' EXIT
slot=$(( $$ % SLOTS ))
{
	flock "$slot_fd"
	st=0
	"$REAL" "$@" || st=$?
	printf '%s\n' "$st" > "$STATUS"
} {slot_fd}< "$SLOTDIR/slot.$slot"
# Bash leaves a {var}< redirection open after the command it was written on, and the detached
# upload below inherits every open descriptor. The lock is released when the last descriptor on it
# closes, so without this the upload holds a COMPILE slot for its whole transfer: measured, one
# upload left slot.0 held after the wrapper had exited, and twelve calls took 4267 ms against 54 ms
# for one, with concurrent uploads pinned at PKG_SLOTS however high their own cap was set.
exec {slot_fd}<&-
code=""
read -r code < "$STATUS" || true
[ -z "$code" ] && exit 2

# Only a compile that succeeded may be served to a later run.
if [ "$code" = 0 ]; then
	tally compiled
	tmp="$entry.$$"
	# Storing an empty directory is what makes a later run restore nothing and call it a hit, so a
	# set that matched no output is thrown away instead.
	mine=("$out_dir"/*"$suffix"*)
	if [ -e "${mine[0]}" ] && mkdir -p "$tmp" &&
		cp -al "${mine[@]}" "$tmp"/ 2>/dev/null &&
		mv -T "$tmp" "$entry" 2>/dev/null; then
		# The upload is detached, because cargo holds this call's job slot until the wrapper exits.
		# An inline upload therefore spends a compile thread on the network, and the entry is
		# already on disk for this build: nothing here waits on the answer.
		#
		# It holds a SHARED lock for its lifetime. `pkg-cache.sh drain` takes the exclusive one,
		# which is what lets the job wait for every upload without counting them.
		#
		# Its own slots cap how many run at once, so a wide outer -j cannot open 16 sockets at a
		# time and earn a 429 that reads as a slow compile.
		if [ -n "$REMOTE" ] && [ -x "$REMOTE" ]; then
			{
				exec {ufd}< "$SLOTDIR/uploads"
				flock -s "$ufd"
				exec {upfd}< "$SLOTDIR/up.$(($$ % UPLOADS))"
				flock "$upfd"
				"$REMOTE" put "$key" "$entry"
				put=$?
				# Counted apart, because a cache service nobody wired up looks exactly like one
				# rejecting every upload, and only one of those is a bug in this script.
				if [ "$put" = 9 ]; then
					tally remote-429
				elif [ "$put" = 3 ]; then
					tally remote-unavailable
				elif [ "$put" = 4 ]; then
					tally remote-finalize-failed
				elif [ "$put" = 0 ]; then
					tally remote-put
				else
					tally remote-put-failed
				fi
			} > /dev/null 2>&1 < /dev/null &
			disown
		fi
	else
		rm -rf "$tmp"
	fi
fi
exit "$code"
