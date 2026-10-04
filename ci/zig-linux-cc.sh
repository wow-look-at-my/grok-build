#!/opt/homebrew/bin/bash
# Local-only helper: drives zig as the C compiler for x86_64-unknown-linux-gnu.
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
