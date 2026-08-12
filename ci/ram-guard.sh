#!/usr/bin/env bash
# RAM regression guard for the compile-time memory pathology of xai-grok-shell
# and xai-grok-pager.
#
# Root cause (diagnosed, see the goal's diagnosis.log): a single rustc that
# monomorphizes a monolith's whole test harness cold-peaks well above the size
# of any *individual* [profile] knob can fix. Measured cold `--lib` harness
# peaks: xai-grok-shell (365k LOC, 4,595 in-lib #[test]s) ~8.9 GiB and
# xai-grok-pager (463k LOC, 8,468 in-lib #[test]s) ~7.3 GiB. Those peaks are
# CGU- and incremental-invariant (they live in type-check/monomorphization), so
# no [profile] setting lowers them. The CI danger is their CONCURRENT sum: when
# the two monolith harnesses overlap each other or other multi-GiB units under
# the cap, the aggregate can exceed the 16 GiB runner limit and the kernel
# SIGKILLs a rustc. The shipped fix isolates the two monolith harnesses out of
# the parallel tier and builds them in an isolated -j1 phase so their aggregate
# never exceeds the larger of the two (~9 GiB).
#
# This guard drives THAT shipped change. It
#   1. FAILS if the monolith-isolation split is removed from
#      .github/workflows/ci.yml (so the pathological crates could overlap again),
#   2. cold-compiles the exact shipped isolated command
#      (`-p xai-grok-shell -p xai-grok-pager --lib --no-run -j1`) and FAILS if
#      the process-tree aggregate peak RSS exceeds BUDGET_GIB. Because -j1 runs
#      the two crates sequentially, the aggregate peak is the larger single
#      harness (~9 GiB), not their sum, and a regression in either crate's
#      compile mass raises it past the budget.
#
# Run where cold RAM headroom exists (local host, or CI with sccache warmed).
set -u
BUDGET_GIB="${BUDGET_GIB:-11}"
BUDGET_BYTES=$((BUDGET_GIB * 1024 * 1024 * 1024))
cd "$(dirname "$0")/.." || exit 2

# 1) Structural check: the shipped CI config must keep the two monoliths out of
#    the parallel tier and build them in an isolated -j1 phase.
fail=0
for crate in xai-grok-shell xai-grok-pager; do
  grep -qF -- "--exclude $crate" .github/workflows/ci.yml \
    || { echo "STRUCTURAL: --exclude ${crate} missing from parallel tier"; fail=1; }
done
grep -qF -- '-p xai-grok-shell -p xai-grok-pager' .github/workflows/ci.yml \
  || { echo "STRUCTURAL: isolated monolith phase missing"; fail=1; }
grep -qF -- '--no-run --no-fail-fast -j1' .github/workflows/ci.yml \
  || { echo "STRUCTURAL: -j1 isolated phase missing"; fail=1; }
if [ "$fail" -ne 0 ]; then echo "RAM GUARD: FAIL — monolith-isolation split not intact"; exit 1; fi
echo "STRUCTURAL: monolith-isolation split intact"

# 2) Cold measurement of the shipped isolated command. Force both monoliths'
#    test targets to recompile so we measure the real monomorphization peak,
#    not a cache replay.
cargo clean -p xai-grok-shell 2>/dev/null || true
cargo clean -p xai-grok-pager 2>/dev/null || true

# Peak RSS of the whole cargo process tree, sampled from /proc. We sum over the
# process GROUP the job belongs to (cargo and every rustc/ld it spawns share
# it), which is exactly what hits the cgroup/runner limit. With -j1 the group
# holds one big rustc at a time, so summing VmRSS does NOT double-count shared
# pages across concurrent rustcs (the over-count the earlier -j3 sample hit).
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

cargo test --locked -p xai-grok-shell -p xai-grok-pager --lib --no-run --no-fail-fast -j1 &
guard_pid=$!
peak_kb=$(measure_tree "$guard_pid")
wait "$guard_pid"; rc=$?
peak_mib=$((peak_kb / 1024))
budget_mib=$((BUDGET_BYTES / 1024 / 1024))
echo "peaked-at: ${peak_mib} MiB | budget: ${budget_mib} MiB | exit: ${rc}"
if [ "$peak_kb" -gt "$BUDGET_BYTES" ]; then
  echo "RAM GUARD: FAIL — monolith --lib harness aggregate peak ${peak_mib} MiB exceeds budget ${budget_mib} MiB"
  exit 1
fi
echo "RAM GUARD: PASS — monolith --lib cold aggregate peak ${peak_mib} MiB within budget ${budget_mib} MiB"
exit 0
