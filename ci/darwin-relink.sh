#!/usr/bin/env bash
# Replays, on macOS, the link that a Linux cross build recorded.
set -euo pipefail

bundle="${1:?usage: darwin-relink.sh <bundle-dir> <output-binary> [expected-format]}"
output="${2:?usage: darwin-relink.sh <bundle-dir> <output-binary> [expected-format]}"
expect="${3:-mach-o-arm64}"

bundle="$(cd "$bundle" && pwd)"
[ -f "$bundle/args" ] || { echo "no recorded link in $bundle" >&2; exit 1; }

# One argument per line.
args=()
# `|| [ -n "$arg" ]` keeps the last line when the file ends without a newline.
while IFS= read -r arg || [ -n "$arg" ]; do
	arg="${arg//@BUNDLE@/$bundle}"
	arg="${arg//@OUT@/$output}"
	args+=("$arg")
done < "$bundle/args"

mkdir -p "$(dirname "$output")"
echo "relinking ${#args[@]} arguments from $bundle"
cc "${args[@]}"

# A link that produced the wrong architecture, or nothing at all, must fail here rather than on a user's Mac.
described="$(file -b "$output")"
echo "$described"
case "$expect:$described" in
	mach-o-arm64:*Mach-O*arm64*) ;;
	elf:*ELF*) ;;
	*) echo "FAIL: expected $expect, got: $described" >&2; exit 1 ;;
esac
chmod +x "$output"
echo "PASS: $expect at $output"
