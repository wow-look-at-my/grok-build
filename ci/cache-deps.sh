#!/usr/bin/env bash
# Cuts a cargo target dir down to the registry artifacts a lockfile determines. DESTRUCTIVE:
# run it after the tests. Usage: cache-deps.sh prune|size <dir>. See AGENTS.md for why a
# workspace artifact is never restorable.
set -euo pipefail

op="${1:?usage: cache-deps.sh prune|size <dir>}"
dir="${2:?usage: cache-deps.sh prune|size <dir>}"

case "$op" in
size)
	printf 'cache-deps: %s is %s\n' "$dir" "$(du -sh "$dir" 2>/dev/null | cut -f1)"
	;;
prune)
	[ -d "$dir" ] || { echo "cache-deps: $dir does not exist" >&2; exit 1; }

	# The manifest is the authority on membership. Matching a name prefix instead would
	# take out any registry crate that happens to start the same way.
	#
	# TARGET names are collected beside package names. Cargo stems a test binary with the
	# target's name and no package anywhere in it, so `pty_e2e_smoke-<hash>` survives a
	# prune that knows only about `xai-grok-pager`. Those binaries are the bulk of the
	# bytes an entry would otherwise carry.
	members="$(cargo metadata --no-deps --format-version 1 --locked |
		jq -r '.packages[] | (.name, .targets[].name)' | sort -u)"
	[ -n "$members" ] || { echo "cache-deps: cargo metadata named no members" >&2; exit 1; }

	before="$(du -sm "$dir" 2>/dev/null | cut -f1)"
	removed=0
	while IFS= read -r name; do
		# .fingerprint and build/ keep the hyphens of the package name. The stems cargo
		# links in deps/ replace them with underscores, and a library also takes a `lib`
		# prefix. The trailing hyphen in each pattern is what stops `xai-grok-pager` from
		# matching `xai-grok-pager-pty-harness`.
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
	;;
*)
	echo "cache-deps: unknown op $op" >&2
	exit 2
	;;
esac
