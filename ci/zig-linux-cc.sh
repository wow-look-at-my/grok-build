#!/opt/homebrew/bin/bash
# Local-only helper: drives zig as the C compiler for x86_64-unknown-linux-gnu
# so `cargo clippy --target x86_64-unknown-linux-gnu` can reach the Linux-only
# source files from a Mac. cc-rs passes its own target selection, which zig
# spells differently, so every target flag is dropped here.
set -euo pipefail

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

exec zig cc -target x86_64-linux-gnu "${args[@]}"
