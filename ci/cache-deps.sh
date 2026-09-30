#!/usr/bin/env bash
# Keys, prunes and restores the dependency cache entry. See AGENTS.md, "The dependency tar".
set -euo pipefail

usage='usage: cache-deps.sh key | prune <dir> <units.json> <stash> | unstash <stash> <dir> | check-fresh <units.json> | size <dir>'
op="${1:?$usage}"

# Cargo names a deps/, build/ or .fingerprint/ entry with the hash its JSON messages print.
entry_hash() {
	[[ "$1" =~ -([0-9a-f]{16})(\.[^/]*)?$ ]] && printf '%s\n' "${BASH_REMATCH[1]}"
}

case "$op" in
key)
	# Only what changes a registry artifact goes in. A lint table or a comment in a manifest changes none.
	echo '::group::cache-deps: key inputs' >&2
	{
		echo '== external packages'
		cargo metadata --locked --format-version 1 |
			jq -c '.workspace_members as $ws | .resolve.nodes[]
				| select(.id as $id | $ws | index($id) | not)
				| {id, features: (.features | sort), deps: ([.deps[].pkg] | sort)}' |
			LC_ALL=C sort
		echo '== rustc'
		rustc -vV
		for file in rust-toolchain.toml .cargo/config.toml; do
			echo "== $file"
			if [ -f "$file" ]; then cat "$file"; fi
		done
		echo '== profile tables'
		awk '/^\[/ { on = ($0 ~ /^\[profile/) } on && !/^[[:space:]]*(#|$)/' Cargo.toml
		echo '== environment'
		env | grep -E '^(CARGO_PROFILE_|CARGO_INCREMENTAL=|RUSTFLAGS=|CARGO_ENCODED_RUSTFLAGS=|CARGO_TARGET_[A-Z0-9_]+_RUSTFLAGS=)' |
			LC_ALL=C sort || true
	} | tee /dev/stderr | sha256sum | cut -c1-40
	echo '::endgroup::' >&2
	;;
prune)
	dir="${2:?$usage}"
	units="${3:?$usage}"
	stash="${4:?$usage}"
	[ -d "$dir" ] || { echo "cache-deps: $dir does not exist" >&2; exit 1; }
	[ -s "$units" ] || { echo "cache-deps: $units is missing or empty" >&2; exit 1; }

	# The manifest is the authority on membership.
	members="$(cargo metadata --no-deps --format-version 1 --locked |
		jq -r '.packages[] | (.name, .targets[].name)' | sort -u)"
	[ -n "$members" ] || { echo "cache-deps: cargo metadata named no members" >&2; exit 1; }

	# Every unit this build used, fresh or compiled. An entry outside this set is left from an older dependency set.
	live="$(jq -r '(.filenames // [])[], (.out_dir // empty), (.executable // empty)' "$units" |
		grep -oE '/(deps|build)/[^/]*-[0-9a-f]{16}' | grep -oE '[0-9a-f]{16}$' | sort -u || true)"
	[ -n "$live" ] || { echo "cache-deps: $units names no unit, so every entry reads as stale" >&2; exit 1; }
	declare -A is_live=()
	while IFS= read -r h; do is_live[$h]=1; done <<<"$live"

	before="$(du -sm "$dir" 2>/dev/null | cut -f1)"
	mkdir -p "$stash"

	# A workspace artifact cannot be restored, so it leaves the entry. The next step needs it, so it moves rather than goes.
	stashed=0
	while IFS= read -r name; do
		# .fingerprint and build/ keep the hyphens of the package name.
		under="${name//-/_}"
		for stem in "$name-" "$under-" "lib$under-"; do
			for sub in .fingerprint build deps; do
				[ -d "$dir/$sub" ] || continue
				while IFS= read -r -d '' entry; do
					# A registry crate whose name only starts with a member's name is not a member.
					rest="${entry##*/}"
					rest="${rest#"$stem"}"
					[[ "$rest" =~ ^[0-9a-f]{16}(\.[^/]*)?$ ]] || continue
					mkdir -p "$stash/$sub"
					mv "$entry" "$stash/$sub/"
					stashed=$((stashed + 1))
				done < <(find "$dir/$sub" -maxdepth 1 -name "$stem*" -print0)
			done
		done
	done <<<"$members"

	stale=0
	for sub in .fingerprint build deps; do
		[ -d "$dir/$sub" ] || continue
		while IFS= read -r -d '' entry; do
			h="$(entry_hash "${entry##*/}")" || continue
			[ -n "${is_live[$h]:-}" ] && continue
			rm -rf "$entry"
			stale=$((stale + 1))
		done < <(find "$dir/$sub" -mindepth 1 -maxdepth 1 -print0)
	done

	after="$(du -sm "$dir" 2>/dev/null | cut -f1)"
	printf 'cache-deps: stashed %d workspace entries, removed %d stale entries, %s MB -> %s MB\n' \
		"$stashed" "$stale" "$before" "$after"

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
unstash)
	stash="${2:?$usage}"
	dir="${3:?$usage}"
	[ -d "$stash" ] || { echo "cache-deps: $stash does not exist, so no prune ran before this" >&2; exit 1; }
	moved=0
	for sub in .fingerprint build deps; do
		[ -d "$stash/$sub" ] || continue
		mkdir -p "$dir/$sub"
		while IFS= read -r -d '' entry; do
			dest="$dir/$sub/${entry##*/}"
			[ -e "$dest" ] && { echo "cache-deps: $dest already exists" >&2; exit 1; }
			mv "$entry" "$dest"
			moved=$((moved + 1))
		done < <(find "$stash/$sub" -mindepth 1 -maxdepth 1 -print0)
	done
	rm -rf "$stash"
	printf 'cache-deps: restored %d workspace entries to %s\n' "$moved" "$dir"
	;;
check-fresh)
	units="${2:?$usage}"
	[ -s "$units" ] || { echo "cache-deps: $units is missing or empty" >&2; exit 1; }
	# After an exact hit, a registry unit that compiled means the key misses an input that changes the build set.
	compiled="$(jq -r 'select(.reason == "compiler-artifact" and .fresh == false)
		| .package_id | select(startswith("path+") | not)' "$units" | sort -u)"
	if [ -n "$compiled" ]; then
		echo "::warning::cache-deps: the key hit, but $(wc -l <<<"$compiled") registry units compiled. Every run recompiles them until the key changes: $(tr '\n' ' ' <<<"$compiled")"
	else
		echo 'cache-deps: the key hit and no registry unit compiled'
	fi
	;;
size)
	dir="${2:?$usage}"
	printf 'cache-deps: %s is %s\n' "$dir" "$(du -sh "$dir" 2>/dev/null | cut -f1)"
	;;
*)
	echo "cache-deps: unknown op $op" >&2
	exit 2
	;;
esac
