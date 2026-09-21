#!/usr/bin/env bash
# Cuts a cargo target dir down to its registry artifacts. Destructive: run it after the tests.
set -euo pipefail

op="${1:?usage: cache-deps.sh prune|size <dir>}"
dir="${2:?usage: cache-deps.sh prune|size <dir>}"

case "$op" in
size)
	printf 'cache-deps: %s is %s\n' "$dir" "$(du -sh "$dir" 2>/dev/null | cut -f1)"
	;;
prune)
	[ -d "$dir" ] || { echo "cache-deps: $dir does not exist" >&2; exit 1; }

	# The manifest is the authority on membership.
	members="$(cargo metadata --no-deps --format-version 1 --locked |
		jq -r '.packages[] | (.name, .targets[].name)' | sort -u)"
	[ -n "$members" ] || { echo "cache-deps: cargo metadata named no members" >&2; exit 1; }

	before="$(du -sm "$dir" 2>/dev/null | cut -f1)"
	removed=0
	while IFS= read -r name; do
		# .fingerprint and build/ keep the hyphens of the package name.
		under="${name//-/_}"
		for stem in "$name-" "$under-" "lib$under-"; do
			for sub in .fingerprint build deps; do
				[ -d "$dir/$sub" ] || continue
				while IFS= read -r -d '' victim; do
					rm -rf "$victim"
					removed=$((removed + 1))
				done < <(find "$dir/$sub" -maxdepth 1 -name "$stem*" -print0)
			done
		done
	done <<<"$members"

	after="$(du -sm "$dir" 2>/dev/null | cut -f1)"
	printf 'cache-deps: pruned %d workspace entries, %s MB -> %s MB\n' \
		"$removed" "$before" "$after"

	# The entry is one tar. Only a per-group size says which group is worth an exclusion.
	# A group whose directory is absent reports zero. Every path here is optional.
	group() {
		label="$1"
		root="$2"
		shift 2
		bytes=0
		if [ -d "$root" ]; then
			bytes="$( (find "$root" "$@" -printf '%s\n' 2>/dev/null || true) |
				awk '{t+=$1} END {printf "%d", t}')"
		fi
		printf 'cache-deps:   %-28s %6d MB\n' "$label" "$((bytes / 1048576))"
	}
	cargo_home="${CARGO_HOME:-$HOME/.cargo}"

	# deps/ is reported by file extension. A hand-named "everything else" bucket
	# hides what it holds, and that bucket was the one worth naming.
	if [ -d "$dir/deps" ]; then
		find "$dir/deps" -maxdepth 1 -type f -printf '%s %f\n' 2>/dev/null |
			awk '{
				ext = "(no extension)"
				if (match($2, /\.[A-Za-z0-9_]+$/)) { ext = substr($2, RSTART) }
				total[ext] += $1
				count[ext] += 1
			}
			END {
				for (e in total) {
					printf "cache-deps:   deps/*%-22s %6d MB  %d files\n",
						e, total[e] / 1048576, count[e]
				}
			}' | sort -k3 -rn
	fi
	group 'build, script binaries' "$dir/build" -type f -name 'build-script-*' ! -name '*.d'
	group 'build, out trees' "$dir/build" -type f -path '*/out/*'
	group 'build, everything else' "$dir/build" -type f \
		! -path '*/out/*' ! -name 'build-script-*'
	group '.fingerprint' "$dir/.fingerprint" -type f
	group 'cargo registry/index' "$cargo_home/registry/index" -type f
	group 'cargo registry/cache' "$cargo_home/registry/cache" -type f
	group 'cargo git/db' "$cargo_home/git/db" -type f
	;;
*)
	echo "cache-deps: unknown op $op" >&2
	exit 2
	;;
esac
