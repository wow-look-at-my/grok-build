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

# Three files, the shape of a real entry: rlib, rmeta and dep-info.
head -c 200000 /dev/urandom > "$src/libprobe-deadbeef.rlib"
printf 'probe-rmeta' > "$src/libprobe-deadbeef.rmeta"
printf 'probe: dep-info\n' > "$src/libprobe-deadbeef.d"

key="probe-$(date +%s)-$$"
export PKG_REMOTE_DEBUG=1

"$here/pkg-remote.sh" put "$key" "$src"
put=$?
echo "PROBE put exit=$put"

"$here/pkg-remote.sh" get "$key" "$dst"
got=$?
echo "PROBE get exit=$got"

if [ "$put" = 0 ] && [ "$got" = 0 ] && diff -r "$src" "$dst" >/dev/null 2>&1; then
	echo "PROBE RESULT pass"
	exit 0
fi
echo "PROBE RESULT fail"
echo "PROBE restored: $(ls -A "$dst" 2>/dev/null | tr '\n' ' ')"
exit 1
