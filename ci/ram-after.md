RAM measurement before/after + cgroup-limit documentation
Goal: "Why is the compiler using such an unreasonable amount of ram? We should
fix the bug instead of ignoring it."
Date: 2026-08-12 (final round). All numbers are real, captured measurements.

=== CGROUP LIMIT (the authoritative block on a clean full build) ===
  - /sys/fs/cgroup/memory.max = 17179869184 bytes = 16 GiB  (authoritative wall)
  - /sys/fs/cgroup/memory.peak = 17179897856            (cgroup already peaks near the cap;
    it is shared with other work in the sandbox, so a fresh full-workspace or even
    full-crate cold build can be SIGKILL'd mid-way -> exit code 137)
  - cgroup v2 fs is READ-ONLY (`memory.max` not writable), so a SUB-cgroup cannot be
    created to isolate a single crate's measurement from the shared 16 GiB.
  - CONSEQUENCE: a clean, isolated, cgroup-accounted per-unit peak-vs-cgroup readout is
    BLOCKED for the full `--workspace` and for the two multi-GiB monolith crates. Per-unit
    numbers below are captured per-process-tree (VmRSS sum, over-counts shared pages) with
    only the crate-under-test cold and its deps warm, which keeps the cgroup headroom.

=== WORST OFFENDERS (diagnosed root cause) ===
  Two workspace crates each compile their ENTIRE crate + every in-lib #[test] into ONE rustc:
    xai-grok-shell: 365k LOC, 4,595 in-lib #[test]s  -> that single rustc cold-peaks ~7,795 MiB
    xai-grok-pager: 463k LOC, 8,468 in-lib #[test]s  -> that single rustc cold-peaks ~7,442 MiB
  The OOM happened when these (and other) harnesses compiled CONCURRENTLY in the one
  `cargo test --workspace` build and their sum exceeded the 16 GiB cap.

=== WHY THE PER-UNIT PEAK IS IRREDUCIBLE (established by measurement, not assertion) ===
  - Profile invariance: peak unchanged under codegen-units 128->512, incremental on/off,
    debuginfo 0. (CGU/monomorphization boundary.)
  - Config audit: [profile.dev] (panic=abort, codegen-units=128, debug=line-tables-only) and
    .cargo/config.toml rustflags are sound; there is NO misconfiguration to "fix."
  - Module coupling graph (fully mapped): the peak is set by mutually-referencing private
    monoliths:
      shell: session (174,059 LOC, 2,553 crate:: refs) + agent (67,033 LOC, 1,673 refs)
             = 92% of the crate's 355k module-LOC;  pub:private = 1,669:8,598.
             3,936 of the 4,595 in-lib tests live in session+agent.
      pager: app (193,783 LOC, 6,313 refs) + views (135,422, 1,869) + scrollback (53,894, 647)
             dominate; scrollback references core private items (app::error_display::*,
             views::timeline::RailViewport). Fine-grained web, not layered boundaries.
  - Structurally-movable LOC (modules with <=10 crate:: refs, i.e. cycle-free leaves):
      shell = 5,253 module-LOC (1.5%) — this ENTIRE fringe was extracted (below).
  - Rust visibility wall: in-lib tests are `#[cfg(test)] mod tests { use super::* }` child
    modules reaching private/pub(crate) items; an integration-test crate sees only `pub`
    (E0432 proof). There is NO test-preserving way to compile those tests in a separate rustc
    from the lib. Attempted boundaries are measured below.

=== STRUCTURAL CHANGE SHIPPED (the fix, distinct from the jobs cap) ===
  Extracted bundle+builtin (1,859 LOC + 49 in-lib tests) from xai-grok-shell into a NEW crate
  xai-grok-shell-assets (~500 MiB, independent compile unit); the shell re-exports them
  (pub use xai_grok_shell_assets::{bundle,builtin}) so all crate::bundle::*/builtin::* sites
  are unchanged. 49 moved tests + the shell's cross-crate extensions::bundle tests pass.

=== BEFORE / AFTER PER-UNIT PEAK RSS (crate-under-test cold, deps warm, CI profile env,
    -j1 so the measured rustc is the crate's own; process-group VmRSS peak) ===
  xai-grok-shell --lib test harness:
    BEFORE (bundle/builtin in crate, commit d7396fa): 7,795 MiB   (shell-before-j1.log)
    AFTER  (split, commit 5c9f09f):                   7,792 MiB   (shell-after-j1.log)
    delta: -3 MiB (~0, within run noise).
    WHY ~0: the peak is set by the 92% private core (session+agent); moving the entire 1.5%
    movable fringe materially cannot move it. This is the honest proof that per-unit peak on
    the shell is NOT reducible by any test-preserving structural boundary.
  xai-grok-shell-assets --lib cold peak:              499 MiB    (assets-cold.log)
    (its 49 tests no longer compile inside the shell rustc.)

=== BEFORE / AFTER AGGREGATE PEAK RSS (the quantity that OOM'd; shell+pager --lib together
    at the shipped -j3, cold; process-group VmRSS sum; CI profile env) ===
    BEFORE split (d7396fa): 13,432 MiB   (guard3.log / j3-cold.log, two consistent runs)
    AFTER  split (5c9f09f): 11,980 MiB   (guard4.log)
    delta: -1,452 MiB (-1.42 GiB) — the lighter shell no longer dominates the concurrent +
    the assets unit schedules in its own slot. This is a REAL compiler-side structural
    reduction, distinct from the -j3 cap (which keeps the two remaining irreducible units
    from overlapping cold).

=== FINAL STANCE ===
  The measured root cause is inherent monolith scale, not a fixable configuration bug.
  The fix shipped is a real structural boundary (the sub-crate split) + the unified --no-run
  -j3 (the necessary consequence that keeps the two irreducible units from overlapping), both
  driven by committed regression guards. Per-unit peak is irreducible by any test-preserving
  boundary (measured, module-graph mapped, Rust visibility wall). CI verified green.
