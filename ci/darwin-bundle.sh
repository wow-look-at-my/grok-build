#!/usr/bin/env bash
# Packs the link that the darwin cross build recorded, for the macOS job to replay.
#
# Usage: ci/darwin-bundle.sh <bundle-dir> <output-tar>
set -euo pipefail

bundle="${1:?usage: darwin-bundle.sh <bundle-dir> <output-tar>}"
tarball="${2:?usage: darwin-bundle.sh <bundle-dir> <output-tar>}"

# An empty bundle means the linker was never invoked, which means cargo found
# the binary fresh and skipped the link. The macOS job would then relink a
# stale bundle, or nothing at all, and neither says so.
if [ ! -f "$bundle/args" ]; then
	echo "no link was recorded in $bundle: the linker never ran" >&2
	exit 1
fi

du -sh "$bundle"
mkdir -p "$(dirname "$tarball")"
tar -cf "$tarball" -C "$bundle" .
ls -lh "$tarball"
