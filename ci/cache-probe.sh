#!/bin/bash
# One put/get round trip against the Actions cache service, in seconds rather than a whole build.
#
# The compile legs answer this question too, but only after half an hour, and they answer it as a
# single counter. This names the step that failed while the failure is still cheap to reproduce.
set -uo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
work="${RUNNER_TEMP:-/tmp}/cache-probe"
src="$work/src"
dst="$work/dst"
rm -rf "$work"
mkdir -p "$src" "$dst"

echo "PROBE cache-url-set ${ACTIONS_RESULTS_URL:+yes}${ACTIONS_RESULTS_URL:-no}"
echo "PROBE cache-token-set ${ACTIONS_RUNTIME_TOKEN:+yes}${ACTIONS_RUNTIME_TOKEN:-no}"
echo "PROBE binpazer $("${BINPAZER:-binpazer}" --version 2>/dev/null || echo MISSING)"
# The toolkit picks v2 only when this is set, and v1 is a different API at a different path.
echo "PROBE cache-service-v2 ${ACTIONS_CACHE_SERVICE_V2:-unset}"
echo "PROBE cache-mode ${ACTIONS_CACHE_MODE:-unset}"
echo "PROBE results-url ${ACTIONS_RESULTS_URL:-unset}"
echo "PROBE cache-url ${ACTIONS_CACHE_URL:-unset}"

# Four files, the shape of a real entry: rlib, rmeta, dep-info, and the build script's own binary.
head -c 200000 /dev/urandom > "$src/libprobe-deadbeef.rlib"
printf 'probe-rmeta' > "$src/libprobe-deadbeef.rmeta"
printf 'probe: dep-info\n' > "$src/libprobe-deadbeef.d"
printf '#!/bin/sh\necho probe\n' > "$src/build-script-build-deadbeef"
chmod 755 "$src/build-script-build-deadbeef"

key="probe-$(date +%s)-$$"
export PKG_REMOTE_DEBUG=1

"$here/pkg-remote.sh" put "$key" "$src"
put=$?
echo "PROBE put exit=$put"

"$here/pkg-remote.sh" get "$key" "$dst"
got=$?
echo "PROBE get exit=$got"

# diff reads content and never a permission, so it passes an entry whose executable came back
# unexecutable. The modes are compared on their own for that reason.
modes() { (cd "$1" && stat -c '%a %n' ./* 2>/dev/null | sort -k2); }
echo "PROBE src-modes $(modes "$src" | tr '\n' ' ')"
echo "PROBE dst-modes $(modes "$dst" | tr '\n' ' ')"

if [ "$put" = 0 ] && [ "$got" = 0 ] && diff -r "$src" "$dst" >/dev/null 2>&1 &&
	[ "$(modes "$src")" = "$(modes "$dst")" ]; then
	echo "PROBE RESULT pass"
	exit 0
fi
echo "PROBE RESULT fail"
echo "PROBE restored: $(ls -A "$dst" 2>/dev/null | tr '\n' ' ')"
exit 1
