#!/usr/bin/env bash
# Startup benchmark. Every startup timing goes through hyperfine, never a hand-rolled timer.
set -euo pipefail

usage="usage: scripts/bench-startup.sh [--binary PATH] [--runs N] [--warmup N] [--out DIR]
  version   the pager's --version: spawn, parse and exit
  tui-cold  a TUI boot into an empty GROK_HOME, up to the first confirmed frame
  tui-warm  the same boot into a GROK_HOME that an earlier boot filled"

repo=$(cd "$(dirname "$0")/.." && pwd)
binary=$repo/target/release/xai-grok-pager
runs=20
warmup=2
out=$repo/target/startup-bench

while [[ $# -gt 0 ]]; do
	case $1 in
	--binary) binary=$2; shift 2 ;;
	--runs) runs=$2; shift 2 ;;
	--warmup) warmup=$2; shift 2 ;;
	--out) out=$2; shift 2 ;;
	-h | --help) echo "$usage"; exit 0 ;;
	*) echo "unknown argument: $1" >&2; exit 2 ;;
	esac
done

command -v hyperfine >/dev/null || { echo "hyperfine is not installed (apt-get install hyperfine)" >&2; exit 1; }
command -v script >/dev/null || { echo "script (util-linux) is not installed" >&2; exit 1; }
[[ -x $binary ]] || { echo "no pager binary at $binary (pass --binary)" >&2; exit 1; }
# The TUI boots run from a sandbox directory, so a relative path stops resolving.
binary=$(cd "$(dirname "$binary")" && pwd)/$(basename "$binary")

sandbox=$(mktemp -d)
trap 'rm -rf "$sandbox"' EXIT
home=$sandbox/home
mkdir -p "$home" "$sandbox/ws" "$out"

# A fixed, offline environment: no inherited GROK_* or endpoint settings reach the boot.
run_env="env -i PATH=$PATH TERM=xterm-256color HOME=$home GROK_HOME=$home/.grok XAI_API_KEY=startup-bench \
GROK_TELEMETRY_ENABLED=false GROK_FEEDBACK_ENABLED=false GROK_PROMPT_SUGGESTIONS=false"
case $(uname -s) in
Darwin) tui="cd $sandbox/ws && $run_env GROK_EXIT_AFTER_FIRST_RENDER=1 script -q /dev/null $binary" ;;
*) tui="cd $sandbox/ws && $run_env GROK_EXIT_AFTER_FIRST_RENDER=1 script -qec $binary /dev/null" ;;
esac

bench() {
	local name=$1
	shift
	hyperfine --runs "$runs" --warmup "$warmup" --command-name "$name" \
		--export-json "$out/$name.json" --export-markdown "$out/$name.md" "$@"
}

bench version "$run_env $binary --version"
bench tui-cold --prepare "rm -rf $home/.grok" "$tui"
bench tui-warm "$tui"

echo "results: $out"
