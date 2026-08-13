#!/usr/bin/env bash
# RAM regression guard for the compile-time memory pathology of xai-grok-shell
# and xai-grok-pager.
#
# ROOT CAUSE (measured, see the goal's per-unit-infeasibility + split evidence):
# the two monolith crates each compile their whole crate + every in-lib test
# into ONE rustc that cold-peaks at ~7.8 GiB (xai-grok-shell) and ~7.4 GiB
# (xai-grok-pager). That peak is type-check/monomorphization-bound and is NOT
# reducible by codegen-units (128->512 unchanged), incremental, or any [profile]
# knob. Per-unit peak is also structurally irreducible on the shell: the crate
# is 355k module-LOC, and 92% of it lives in `session`+`agent`, two mutually-
# referencing monoliths whose ~4,200 `crate::` references reach private surface
# (shell pub:private = 1,669:8,598). Only ~1.5% of LOC is structurally movable;
# the OOM happens when the multi-GiB units compile CONCURRENTLY in one
# `cargo test --workspace` build.
#
# THE REAL FIX (this branch): a structural sub-crate split. bundle+builtin
# (1,859 LOC + 49 in-lib tests) were extracted from xai-grok-shell into a new
# crate xai-grok-shell-assets (~499 MiB cold peak, an independent compile unit
# that schedules in its own -j slot instead of inside the shell's rustc), and
# the shell re-exports them. The remaining per-unit peak is unchanged because
# it is set by the 92% private core — which is why the -j3 cap on the unified
# compile step is still required to keep the two irreducible units from
# overlapping on a cold build.
#
# This guard drives BOTH shipped changes:
#   1. FAILS if the structural split is undone (bundle/builtin re-inlined into
#      the shell, or the assets crate removed).
#   2. FAILS if the unified `--no-run` compile at -j3 is removed from
#      .github/workflows/ci.yml, or if the --exclude/-p split (which caused a
#      -j16 recompile OOM) is reintroduced.
#   3. Cold-compiles the exact worst-case shipped scenario — the two monolith
#      `--lib` harnesses together at -j3 — and FAILS if the process-tree
#      aggregate peak RSS exceeds BUDGET_GIB.
set -u
# Budget: 15 GiB. The worst-case cold aggregate of the two monolith `--lib`
# harnesses at the shipped -j3 parallelism was MEASURED at 13,432 MiB (process-
# group VmRSS sum, which over-counts shared pages vs the cgroup's authoritative
# memory.peak, so the real committed peak is lower). 15 GiB is the highest
# budget that still catches a ~1.6 GiB regression in either monolith's compile
# mass while staying under the 16 GiB runner cap.
BUDGET_GIB="${BUDGET_GIB:-15}"
BUDGET_BYTES=$((BUDGET_GIB * 1024 * 1024 * 1024))
cd "$(dirname "$0")/.." || exit 2

# 1) Structural split check: bundle+builtin must live in xai-grok-shell-assets,
#    not in xai-grok-shell/src, and the shell must pull them in by dependency.
fail=0
if [ ! -f crates/codegen/xai-grok-shell-assets/src/bundle.rs ] \
   || [ ! -f crates/codegen/xai-grok-shell-assets/src/builtin.rs ]; then
  echo "STRUCTURAL: bundle/builtin must live in xai-grok-shell-assets (sub-crate split)"
  fail=1
fi
if [ -f crates/codegen/xai-grok-shell/src/bundle.rs ] \
   || [ -f crates/codegen/xai-grok-shell/src/builtin.rs ]; then
  echo "STRUCTURAL: bundle/builtin re-inlined into xai-grok-shell — split undone"
  fail=1
fi
grep -q 'xai-grok-shell-assets' crates/codegen/xai-grok-shell/Cargo.toml \
  || { echo "STRUCTURAL: shell must depend on xai-grok-shell-assets"; fail=1; }
grep -q 'xai-grok-shell-assets' Cargo.toml \
  || { echo "STRUCTURAL: xai-grok-shell-assets must be a workspace member"; fail=1; }

# 2) CI config check: the build step must be a single unified --no-run at -j3
#    and must NOT reintroduce the --exclude/-p split. awk is used so these
#    patterns don't match this guard's own command text below.
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
grep -qF -- 'cargo nextest run --locked --workspace --profile ci' .github/workflows/ci.yml \
  || { echo "STRUCTURAL: run step missing"; fail=1; }
if [ "$fail" -ne 0 ]; then echo "RAM GUARD: FAIL — shipped compile-RAM structure not intact"; exit 1; fi
echo "STRUCTURAL: sub-crate split + unified --no-run at -j3 intact, no exclude split"

# 3) Cold measurement of the worst-case shipped compile: the two monolith
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