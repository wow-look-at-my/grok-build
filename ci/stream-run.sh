#!/bin/bash
# Runs a command with its output streamed to log-streamer, and runs it anyway when the streamer
# is not reachable.
#
# The org's stream action makes the client download required, so one 404 from the registry ends
# the job before the command starts. That cost a whole measurement run: three legs and the probe
# died in under a minute having compiled nothing. Realtime logs are a convenience, and a
# convenience must never be able to fail a measurement.
#
#   stream-run.sh <stream-name> <command...>
set -uo pipefail

name="${1:-}"
shift || true
[ -n "$name" ] && [ "$#" -gt 0 ] || exit 2

client="${RUNNER_TEMP:-/tmp}/ls-client"
url="https://dl.pazer.build/log-streamer/ls-client?os=linux&arch=amd64"

have_client=
if [ -n "${LOG_STREAMER_STREAM_KEY:-}" ]; then
	for attempt in 1 2 3 4 5; do
		if curl -fsSL --max-time 60 "$url" -o "$client" && chmod +x "$client"; then
			have_client=1
			break
		fi
		echo "stream-run: client download attempt $attempt failed" >&2
		sleep "$attempt"
	done
fi

if [ -n "$have_client" ]; then
	if token="$("$client" token derive --name "$name" 2>/dev/null)" && [ -n "$token" ]; then
		echo "::add-mask::$token"
		script="${RUNNER_TEMP:-/tmp}/stream-run-$$.sh"
		printf '%s\n' "$*" > "$script"
		LOG_STREAMER_SERVER="${LOG_STREAMER_SERVER:-wss://logs.pazer.io}" \
			LOG_STREAMER_TOKEN="$token" "$client" shell "$script"
		exit $?
	fi
	echo "stream-run: token derive failed, running unstreamed" >&2
else
	echo "stream-run: no client, running unstreamed" >&2
fi

"$@"
