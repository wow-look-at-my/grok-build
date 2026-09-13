#!/bin/bash
# Offline checks for `pkg-remote.sh get`. No cache service and no network beyond loopback.
#
# The invariant under test is the one whose loss broke the build: a get that fails partway must
# leave NO entry behind. The wrapper's restore reads any non-empty entry directory as a hit, so a
# partial one is served to cargo as a finished package, and every dependent then reports
# `E0463: can't find crate` for a crate the same build is compiling.
set -uo pipefail

HERE="${0%/*}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"; [ -n "${SERVER_PID:-}" ] && kill "$SERVER_PID" 2>/dev/null' EXIT

fail() {
	echo "FAIL: $1" >&2
	exit 1
}

# A stub of the two calls `get` makes: the v1 lookup that answers with an archive location, and the
# archive itself. The archive is deliberately not a binpazer file, so the extract below fails the
# way a truncated or corrupted download does.
cat > "$WORK/stub.js" <<'JS'
const http = require('http');
const server = http.createServer((req, res) => {
	if (req.url.startsWith('/_apis/artifactcache/cache')) {
		const port = server.address().port;
		res.setHeader('content-type', 'application/json');
		res.end(JSON.stringify({ archiveLocation: `http://127.0.0.1:${port}/blob` }));
		return;
	}
	res.end('not a binpazer archive');
});
server.listen(0, '127.0.0.1', () => console.log(server.address().port));
JS

node "$WORK/stub.js" > "$WORK/port" &
SERVER_PID=$!
for _ in $(seq 1 50); do
	[ -s "$WORK/port" ] && break
	sleep 0.1
done
read -r PORT < "$WORK/port" || fail "stub server did not report a port"
[ -n "$PORT" ] || fail "stub server did not report a port"

export ACTIONS_CACHE_URL="http://127.0.0.1:$PORT/"
export ACTIONS_RESULTS_URL=""
export ACTIONS_RUNTIME_TOKEN="stub-token"

# A binpazer that names two artifacts, writes the first, and fails on the second. This is the shape
# that actually breaks a build: an entry holding the rmeta and not the rlib passes the wrapper's
# "is there anything here" restore, so cargo is handed a package that was never linked.
cat > "$WORK/binpazer-partial" <<'SH'
#!/bin/bash
out=""
type=""
index=""
prev=""
for a in "$@"; do
	case "$prev" in
	-o) out="$a"; prev=""; continue ;;
	--type) type="$a"; prev=""; continue ;;
	--index) index="$a"; prev=""; continue ;;
	esac
	case "$a" in
	-o | --type | --index) prev="$a" ;;
	esac
done
if [ "$type" = 2 ]; then
	printf '644 libfoo-abc123.rmeta\n644 libfoo-abc123.rlib\n' > "$out"
	exit 0
fi
# The rlib is the one that never arrives.
[ "$index" = 0 ] || exit 1
printf 'rmeta' > "$out"
SH
chmod +x "$WORK/binpazer-partial"

check_leaves_nothing() {
	local label="$1" entry="$2"
	"$HERE/pkg-remote.sh" get deadbeef "$entry"
	local rc=$?
	[ "$rc" = 0 ] && fail "$label: a get whose extract failed reported success"
	if [ -e "$entry" ]; then
		echo "left behind:" >&2
		ls -la "$entry" >&2
		fail "$label: a failed get left an entry at $entry; the wrapper reads that as a hit"
	fi
	local leftovers=("${entry%/*}"/*.part)
	[ -e "${leftovers[0]}" ] && fail "$label: a failed get left its partial directory behind"
	return 0
}

# The rlib is missing, so the entry must not survive at all.
BINPAZER="$WORK/binpazer-partial" check_leaves_nothing "partial extract" "$WORK/store-partial/deadbeef"
# Nothing extracts at all, which is the truncated-download case.
BINPAZER=false check_leaves_nothing "failed extract" "$WORK/store-empty/deadbeef"

echo "ok: a failed get leaves no entry"
