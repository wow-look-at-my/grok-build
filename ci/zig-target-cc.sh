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

# zig's clang does not read SDKROOT the way a Darwin-hosted clang does, so
# aws-lc-sys failed on a missing CoreServices/CoreServices.h with the SDK
# sitting right there. -isysroot is headers and frameworks only: it never
# rewrites a linker search path, which is the trap that sank linking here.
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
# winning. The SDK only fills what zig does not carry, such as the libDER that
# Security.framework's oids.h includes.
exec zig "$mode" -target aarch64-macos \
	-isysroot "$sdk" \
	-iframework "$sdk/System/Library/Frameworks" \
	-idirafter "$sdk/usr/include" \
	"${args[@]}"
