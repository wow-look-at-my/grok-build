#!/usr/bin/env bash
# Drives zig as the C compiler for aarch64-apple-darwin during the Linux half of the darwin build.
set -euo pipefail

mode="${1:?usage: zig-target-cc.sh <cc|c++> [args...]}"
shift

# zig's clang does not read SDKROOT the way a Darwin-hosted clang does.
sdk="${SDKROOT:?SDKROOT must name the macOS SDK for the darwin cross build}"

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

# -idirafter, not -I: zig ships its own macOS libc headers and they must keep
# winning.
exec zig "$mode" -target aarch64-macos \
	-isysroot "$sdk" \
	-iframework "$sdk/System/Library/Frameworks" \
	-idirafter "$sdk/usr/include" \
	"${args[@]}"
