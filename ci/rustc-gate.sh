#!/bin/bash
# Stands in for rustc so that only REAL compiles are rationed.
#
# Cargo has one -j, and it governs cache hits and real compiles alike. A hit costs a lookup and no
# memory, so pinning the whole build to the number of compiles memory allows throttles the case
# that needs no throttling: at a full cache this workspace's compile step was about 1800 lookups
# run three at a time.
#
# sccache is what separates them, because it execs the compiler only when it is going to compile.
# A hit is answered from the cache and never reaches this script. So cargo runs wide, sccache
# absorbs the hits, and the semaphore below holds the compiles that are left. Measured on a probe
# crate: a cold pass reached this script for 30 crate compiles, a warm pass for 21, and the
# difference was exactly the 9 cache misses.
#
# Wiring, which is what makes sccache the caller rather than the callee:
#   RUSTC_WRAPPER=sccache   RUSTC=ci/rustc-gate.sh   GATE_REAL_RUSTC=$(rustup which rustc)
# Cargo runs `$RUSTC_WRAPPER $RUSTC <args>`, so sccache receives this script as its compiler.
set -uo pipefail

# The fallback is the rustup shim on PATH. Cargo's RUSTC points here, not there, so the shim
# cannot re-enter this script. Naming the toolchain binary in GATE_REAL_RUSTC skips the shim.
REAL="${GATE_REAL_RUSTC:-rustc}"
SLOTS="${GATE_SLOTS:-3}"
DIR="${GATE_SLOT_DIR:-${RUNNER_TEMP:-/tmp}/rustc-gate}"

# A probe reads a value out of the compiler and allocates nothing worth rationing. Cargo issues
# these constantly, and making them queue behind a compile would serialize the whole build plan.
for arg in "$@"; do
	case "$arg" in
	--print | --print=* | --version | -vV)
		exec "$REAL" "$@"
		;;
	esac
done

mkdir -p "$DIR"

# flock runs the compiler while holding the slot and the kernel drops the lock when that process
# ends, however it ends. A killed compile therefore cannot strand a slot nothing owns.
#
# --conflict-exit-code is what separates "the slot was busy" from "the compiler exited 1". Without
# it a failing compile reads as contention and the build silently retries it on the next slot.
BUSY=99
while :; do
	for ((i = 0; i < SLOTS; i++)); do
		flock --nonblock --conflict-exit-code "$BUSY" "$DIR/slot.$i" "$REAL" "$@"
		rc=$?
		if [ "$rc" -ne "$BUSY" ]; then
			exit "$rc"
		fi
	done
	sleep 0.05
done
