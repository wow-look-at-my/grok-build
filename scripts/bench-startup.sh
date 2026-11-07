#!/usr/bin/env bash
# Startup benchmark. Every startup timing goes through hyperfine, never a hand-rolled timer.
set -euo pipefail

usage="usage: scripts/bench-startup.sh [pager|watchers] [options]

pager (the default) times the pager binary:
  version     --version: spawn, parse and exit
  tui-cold    a TUI boot into an empty GROK_HOME, up to the first confirmed frame
  tui-warm    the same boot into a GROK_HOME that an earlier boot filled
  tui-resume  a --continue boot of a session that startup-resume-seed filled with turns
  options: --binary PATH (default target/release/xai-grok-pager)
           --seed-binary PATH (default target/debug/startup-resume-seed)

watchers times one watcher start per process, built from the crates' examples:
  fsnotify-<scenario>       FsEventSource::start on a scenario tree (xai-fsnotify watcher_startup)
  skills-scoped             SkillsFileWatcher::start_with_dirs on a .claude with a large worktrees/ tree
  skills-recursive-control  one recursive watch on the same .claude
  options: --worktree-dirs N (default 6000), --debug (build the examples unoptimized),
           --examples-dir DIR (use prebuilt examples there instead of building them)

both: --runs N (default 20), --warmup N (default 2), --out DIR (default target/startup-bench)"

repo=$(cd "$(dirname "$0")/.." && pwd)
suite=pager
if [[ $# -gt 0 && $1 != -* ]]; then
	suite=$1
	shift
fi
binary=$repo/target/release/xai-grok-pager
seed_binary=$repo/target/debug/startup-resume-seed
runs=20
warmup=2
out=$repo/target/startup-bench
worktree_dirs=6000
profile=release
examples_dir=

while [[ $# -gt 0 ]]; do
	case $1 in
	--binary) binary=$2; shift 2 ;;
	--seed-binary) seed_binary=$2; shift 2 ;;
	--runs) runs=$2; shift 2 ;;
	--warmup) warmup=$2; shift 2 ;;
	--out) out=$2; shift 2 ;;
	--worktree-dirs) worktree_dirs=$2; shift 2 ;;
	--debug) profile=debug; shift ;;
	--examples-dir) examples_dir=$2; shift 2 ;;
	-h | --help) echo "$usage"; exit 0 ;;
	*) echo "unknown argument: $1" >&2; exit 2 ;;
	esac
done

command -v hyperfine >/dev/null || { echo "hyperfine is not installed (apt-get install hyperfine)" >&2; exit 1; }

sandbox=$(mktemp -d)
trap 'rm -rf "$sandbox"' EXIT
mkdir -p "$out"

bench() {
	local name=$1
	shift
	hyperfine --runs "$runs" --warmup "$warmup" --command-name "$name" \
		--export-json "$out/$name.json" --export-markdown "$out/$name.md" "$@"
}

absolute() {
	echo "$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
}

pager_suite() {
	command -v script >/dev/null || { echo "script (util-linux) is not installed" >&2; exit 1; }
	[[ -x $binary ]] || { echo "no pager binary at $binary (pass --binary)" >&2; exit 1; }
	[[ -x $seed_binary ]] || {
		echo "no seeder at $seed_binary: cargo build -p xai-grok-pager-pty-harness --bin startup-resume-seed, or pass --seed-binary" >&2
		exit 1
	}
	# The TUI boots run from a sandbox directory, so a relative path stops resolving.
	binary=$(absolute "$binary")
	seed_binary=$(absolute "$seed_binary")
	local home=$sandbox/home ws=$sandbox/ws
	mkdir -p "$home" "$ws"

	bench version "$(boot_env "$home/.grok") $binary --version"
	bench tui-cold --prepare "rm -rf $home/.grok" "$(tui_boot "$home/.grok" "$ws")"
	bench tui-warm "$(tui_boot "$home/.grok" "$ws")"

	local resume_home=$sandbox/resume-home
	"$seed_binary" --binary "$binary" --cwd "$ws" --grok-home-out "$resume_home"
	bench tui-resume "$(tui_boot "$resume_home" "$ws" --continue)"
}

# A fixed, offline environment: no inherited GROK_* or endpoint settings reach the boot.
boot_env() {
	echo "env -i PATH=$PATH TERM=xterm-256color HOME=$sandbox/home GROK_HOME=$1 XAI_API_KEY=startup-bench" \
		"GROK_TELEMETRY_ENABLED=false GROK_FEEDBACK_ENABLED=false GROK_PROMPT_SUGGESTIONS=false GROK_EXIT_AFTER_FIRST_RENDER=1"
}

# The boot stops at the first confirmed frame. `script` gives it a pty, because hyperfine gives its command no terminal.
tui_boot() {
	local grok_home=$1 ws=$2
	shift 2
	local cmd="$binary${*:+ $*}"
	case $(uname -s) in
	Darwin) echo "cd $ws && $(boot_env "$grok_home") script -q /dev/null $cmd" ;;
	*) echo "cd $ws && $(boot_env "$grok_home") script -qec '$cmd' /dev/null" ;;
	esac
}

watchers_suite() {
	if [[ -z $examples_dir ]]; then
		local build_flags=()
		[[ $profile == release ]] && build_flags+=(--release)
		cargo build "${build_flags[@]}" -p xai-fsnotify --example watcher_startup
		cargo build "${build_flags[@]}" -p xai-grok-shell --example skills_watcher_startup
		examples_dir=$repo/target/$profile/examples
	fi
	local fsnotify=$examples_dir/watcher_startup
	local skills=$examples_dir/skills_watcher_startup
	[[ -x $fsnotify && -x $skills ]] || { echo "watcher_startup or skills_watcher_startup missing in $examples_dir" >&2; exit 1; }

	local scenario dir
	for scenario in $("$fsnotify" scenarios); do
		dir=$sandbox/fsnotify-$scenario
		bench "fsnotify-$scenario" --setup "rm -rf $dir && $fsnotify fixture $scenario $dir" "$fsnotify start $dir"
	done

	dir=$sandbox/skills
	bench skills-scoped --setup "rm -rf $dir && $skills fixture $worktree_dirs $dir" "$skills scoped $dir"
	bench skills-recursive-control "$skills recursive-control $dir"
}

case $suite in
pager) pager_suite ;;
watchers) watchers_suite ;;
*) echo "unknown suite: $suite" >&2; echo "$usage" >&2; exit 2 ;;
esac

echo "results: $out"
