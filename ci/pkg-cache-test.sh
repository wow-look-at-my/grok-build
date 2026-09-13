#!/bin/bash
# Offline checks for the wrapper's restore. No cache service, no network, no compiler.
#
# The invariant: a hit must carry everything the entry was stored with. An entry missing one of its
# files reads as a miss, because serving it tells cargo a package is built when its rlib is absent,
# and every dependent then fails with `E0463: can't find crate`.
set -uo pipefail

HERE="${0%/*}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

fail() {
	echo "FAIL: $1" >&2
	exit 1
}

export PKG_CACHE_DIR="$WORK/store"
export PKG_SLOT_DIR="$WORK/slots"
export PKG_STATS_DIR="$WORK/stats"
export PKG_NO_REMOTE=1
mkdir -p "$PKG_CACHE_DIR" "$WORK/out"

# A rustc that writes the two files cargo expects of a lib, so the wrapper has something to store.
cat > "$WORK/rustc" <<'SH'
#!/bin/bash
out=""
prev=""
for a in "$@"; do
	case "$prev" in
	--out-dir) out="$a"; prev="" ; continue ;;
	esac
	case "$a" in
	--out-dir) prev="--out-dir" ;;
	esac
done
printf 'rlib' > "$out/libfoo-abc123.rlib"
printf 'rmeta' > "$out/libfoo-abc123.rmeta"
SH
chmod +x "$WORK/rustc"

printf 'fn main() {}\n' > "$WORK/foo.rs"
compile() {
	"$HERE/pkg-cache.sh" "$WORK/rustc" --crate-name foo "$WORK/foo.rs" \
		-C extra-filename=-abc123 --out-dir "$1"
}

# First call compiles and stores.
compile "$WORK/out" || fail "the first call did not succeed"
entries=("$PKG_CACHE_DIR"/*)
[ -e "${entries[0]}" ] || fail "the first call stored nothing"
entry="${entries[0]}"
[ -f "$entry/pkg-files.list" ] || fail "a stored entry carries no list of what it holds"

# Second call into a fresh directory must be served from the store, whole.
mkdir -p "$WORK/out2"
compile "$WORK/out2" || fail "the second call did not succeed"
for name in libfoo-abc123.rlib libfoo-abc123.rmeta; do
	[ -f "$WORK/out2/$name" ] || fail "a hit did not restore $name"
done
# The list is bookkeeping and must not land in the build directory.
[ -e "$WORK/out2/pkg-files.list" ] && fail "the entry's own list reached the build directory"

# Now break the entry the way a truncated fetch or a trimmed store does, and ask again.
rm -f "$entry/libfoo-abc123.rlib"
mkdir -p "$WORK/out3"
compile "$WORK/out3" || fail "the third call did not succeed"
[ -f "$WORK/out3/libfoo-abc123.rlib" ] ||
	fail "an entry missing its rlib was served as a hit; cargo would see a package that was never linked"

echo "ok: a hit carries every file its entry was stored with"
