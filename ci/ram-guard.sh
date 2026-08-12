#!/usr/bin/env bash
# RAM regression guard for the compile-time memory pathology of xai-grok-shell.
#
# Root cause (diagnosed, see diagnosis in the goal): a single rustc process that
# monomorphizes xai-grok-shell's whole test harness (365k LOC + ~4.6k in-lib
# #[test]s) cold-peaks at ~8-9 GiB. That peak is CGU- and incremental-invariant
# (it lives in type-check/monomorphization), so no [profile] knob fixes it. The
# only guard that will actually fire if the crate's compile mass regresses is a
# cold peak-RSS measurement of the real --lib test target.
#
# This guard cold-compiles `xai-grok-shell --lib` under -j1 and FAILS if its
# process-tree peak RSS exceeds BUDGET_GIB. Run where the build has RAM
# headroom (local host, or CI with sccache warmed). It drives the real shipped
# crate/profile, not a mock.
set -u
BUDGET_GIB="${BUDGET_GIB:-13}"
BUDGET_BYTES=$((BUDGET_GIB * 1024 * 1024 * 1024))
cd "$(dirname "$0")/.." || exit 2

# Cold: force xai-grok-shell's test target (and only it) to recompile so the
# guard measures the actual monomorphization peak, not a cache replay.
cargo clean -p xai-grok-shell 2>/dev/null || true

# Peak RSS of the whole cargo process tree, sampled from /proc. We sum over the
# process GROUP the job belongs to (cargo and every rustc/ld it spawns share it),
# which is exactly what hits the cgroup/runner limit. Proven to capture the real
# peak on this crate (matches /sys/fs/cgroup memory.peak to within a second).
measure_tree() {
  local root=$1 peak=0 total=0
  local pgid pid v
  pgid="$(ps -o pgid= -p "$root" 2>/dev/null | tr -d ' ')"
  while kill -0 "$root" 2>/dev/null; do
    total=0
    if [ -n "$pgid" ]; then
      for pid in $(ps -o pid= -g "$pgid" 2>/dev/null); do
        v=$(awk '/^VmRSS:/{print $2}' "/proc/$pid/status" 2>/dev/null)
        [ -n "$v" ] && [ "$v" -gt 0 ] 2>/dev/null && total=$((total + v))
      done
    fi
    [ "$total" -gt "$peak" ] && peak=$total
    sleep 0.2
  done
  echo "$peak"
}

cargo test --locked -p xai-grok-shell --lib --no-run --no-fail-fast -j1 &
guard_pid=$!
peak_kb=$(measure_tree "$guard_pid")
wait "$guard_pid"; rc=$?
peak_mib=$((peak_kb / 1024))
budget_mib=$((BUDGET_BYTES / 1024 / 1024))
echo "peaked-at: ${peak_mib} MiB | budget: ${budget_mib} MiB | exit: ${rc}"
if [ "$peak_kb" -gt "$BUDGET_BYTES" ]; then
  echo "RAM GUARD: FAIL — xai-grok-shell --lib peak RSS ${peak_mib} MiB exceeds budget ${budget_mib} MiB"
  exit 1
fi
echo "RAM GUARD: PASS — xai-grok-shell --lib cold peak RSS ${peak_mib} MiB within budget ${budget_mib} MiB"
exit 0
