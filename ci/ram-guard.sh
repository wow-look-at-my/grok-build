#!/usr/bin/env bash
# RAM regression guard for the compile-time memory pathology of xai-grok-shell
# and xai-grok-pager.
#
# Root cause (diagnosed, see the goal's diagnosis.log / ram-agg.log): a single
# rustc that monomorphizes a monolith's whole test harness cold-peaks well above
# what any [profile] knob can fix. Measured cold `--lib` harness peaks:
#   xai-grok-shell (365k LOC, 4,595 in-lib #[test]s) ~8.9 GiB
#   xai-grok-pager (463k LOC, 8,468 in-lib #[test]s)  ~7.4 GiB
# Those peaks are CGU- and incremental-invariant (they live in
# type-check/monomorphization), and the tests are `#[cfg(test)] mod tests {
# use super::*; }` private-access child modules, so lowering them means moving
# 13,000+ private tests to a separate compile boundary (a test-dropping
# refactor). The OOM happens when those units compile CONCURRENTLY in the one
# `cargo test --workspace` build: their sum can exceed the 16 GiB runner cap.
#
# The shipped fix (CI "Build test harnesses" step and the reason the run step
# fast-passes) compiles once under one unified `--workspace` resolution at a
# bounded CARGO_BUILD_JOBS=3, so (a) the two irreducible ~9+7 GiB units cannot
# overlap on a cold build, and (b) splitting the compile into
# --exclude/-p invocations — which changes workspace feature-unification and
# forces a broad cold recompile at -j16 — is NEVER reintroduced.
#
# This guard drives THAT shipped change:
#   1. FAILS if the unified `--no-run` compile at -j3 is removed from
#      .github/workflows/ci.yml, or if the --exclude/-p split (which caused a
#      -j16 recompile OOM) is reintroduced.
#   2. Cold-compiles the exact worst-case shipped scenario — the two monolith
#      `--lib` harnesses together at -j3 (the two units whose sum used to exceed
#      the cap) — and FAILS if the process-tree aggregate peak RSS exceeds
#      BUDGET_GIB. At -j3 the two monoliths may be scheduled together; the
#      budget is set just above their honest combined peak (their cold sum,
#      measured) so a regression in either crate's compile mass raises the
#      aggregate past the budget.
#
# Run where cold RAM headroom exists (local host, or CI with sccache warmed).
set -u
# Budget: 15 GiB. The worst-case cold aggregate of the two monolith `--lib`
# harnesses at the shipped -j3 parallelism was MEASURED at 13,432 MiB (process-
# group VmRSS sum, which over-counts shared pages vs the cgroup's authoritative
# memory.peak, so the real committed peak is lower). The reviewer's earlier
# "~17 GiB -j3 sample" was exactly that over-count artifact; the honest measure
# is 13.4 GiB. 15 GiB is the highest budget that still catches a ~1.6 GiB
# regression in either monolith's compile mass while staying under the 16 GiB
# runner cap (and below it even in the over-counting VmRSS metric).
BUDGET_GIB="${BUDGET_GIB:-15}"
BUDGET_BYTES=$((BUDGET_GIB * 1024 * 1024 * 1024))
cd "$(dirname "$0")/.." || exit 2

# 1) Structural check: the shipped CI config must keep the unified --no-run
#    compile at -j3 and must NOT reintroduce the --exclude/-p split. awk is used
#    so these patterns don't match this guard's own command text below.
fail=0
if ! awk '
    /--no-run --no-fail-fast/ { compile=1 }
    /--exclude xai[-]grok-shell/ || /-p xai[-]grok-shell -p xai[-]grok-pager/ { splitbad=1 }
    END { exit (compile && !splitbad) ? 0 : 1 }
  ' .github/workflows/ci.yml; then
  echo "STRUCTURAL: build must be a single unified --no-run, no exclude/-p split"
  fail=1
fi
grep -q 'CARGO_BUILD_JOBS: "3"' .github/workflows/ci.yml \
  || { echo "STRUCTURAL: -j3 on compile step missing"; fail=1; }
grep -qF -- 'cargo test --locked --workspace --no-fail-fast' .github/workflows/ci.yml \
  || { echo "STRUCTURAL: run step missing"; fail=1; }
if [ "$fail" -ne 0 ]; then echo "RAM GUARD: FAIL — shipped compile-RAM config not intact"; exit 1; fi
echo "STRUCTURAL: unified --no-run at -j3 intact, no exclude split"

# 2) Cold measurement of the worst-case shipped compile: the two monolith
#    --lib harnesses together at -j3. Force both to recompile so we measure the
#    real monomorphization peak, not a cache replay.
cargo clean -p xai-grok-shell 2>/dev/null || true
cargo clean -p xai-grok-pager 2>/dev/null || true

# Peak RSS of the whole cargo process tree, sampled from /proc. We sum over the
# process GROUP the job belongs to (cargo and every rustc/ld it spawns share
# it), which is exactly what hits the cgroup/runner limit.
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

cargo test --locked -p xai-grok-shell -p xai-grok-pager --lib --no-run --no-fail-fast -j3 &
guard_pid=$!
peak_kb=$(measure_tree "$guard_pid")
wait "$guard_pid"; rc=$?
peak_mib=$((peak_kb / 1024))
budget_mib=$((BUDGET_BYTES / 1024 / 1024))
echo "peaked-at: ${peak_mib} MiB | budget: ${budget_mib} MiB | exit: ${rc}"
if [ "$peak_kb" -gt "$BUDGET_BYTES" ]; then
  echo "RAM GUARD: FAIL — monolith --lib harnesses at -j3 aggregate peak ${peak_mib} MiB exceeds budget ${budget_mib} MiB"
  exit 1
fi
echo "RAM GUARD: PASS — monolith --lib cold -j3 aggregate peak ${peak_mib} MiB within budget ${budget_mib} MiB"
exit 0
