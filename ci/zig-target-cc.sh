#!/usr/bin/env bash
# Drives zig as the C compiler for aarch64-apple-darwin during the Linux half
# of the darwin build.
#
# cc-rs adds `--target=arm64-apple-macosx` of its own. zig does not know
# `arm64` as an architecture name and answers "unknown architecture: 'arm64'",
# which failed every C and assembly file in ring and aws-lc. So every target
# selection cc-rs passes is dropped here and zig's own spelling is used.
#
# Usage: ci/zig-target-cc.sh <cc|c++> [compiler args...]
set -euo pipefail

mode="${1:?usage: zig-target-cc.sh <cc|c++> [args...]}"
shift

args=()
skip_next=0
for arg in "$@"; do
	if [ "$skip_next" = 1 ]; then
		skip_next=0
		continue
	fi
	case "$arg" in
		--target=*) ;;
		-target) skip_next=1 ;;
		*) args+=("$arg") ;;
	esac
done

exec zig "$mode" -target aarch64-macos "${args[@]}"
